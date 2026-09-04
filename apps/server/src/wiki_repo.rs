//! The server's half of repo-sourced wikis (`wiki.source.*`).
//!
//! `wiki_live::repo_source` knows how to sync a mirror and push a
//! branch; it does not know when, and it cannot open a pull request
//! because the forge clients live above it in the dependency graph.
//! Both of those are here:
//!
//! - [`spawn_sync_loop`] — one task per org that keeps every
//!   repo-sourced wiki current on a schedule, so a commit upstream
//!   becomes wiki content without a person re-importing anything
//!   (`wiki.source.sync`). The first pass runs at boot, off the boot
//!   path.
//! - [`open_pull_request`] — turns a pushed landing branch into a pull
//!   request on whichever forge the repository's URL names, with the
//!   token the deployment holds for it. No token means no pull request
//!   — the branch is still pushed, and the caller says so
//!   (`wiki.source.editable`).

use std::time::Duration;

use wiki_live::edits_backend::{EditsBackend, ForgeIdentity, Lander};
use wiki_live::repo_source::Landing;
use wiki_proto::config::RepoSource;
use wiki_proto::error::WikiError;
use wiki_proto::service::registry::Registry as _;

/// The server's [`Lander`]: a pushed landing branch becomes a pull
/// request on the repository's forge through [`open_pull_request`].
///
/// On GitHub the identity is the accepting Editor's own: the issuer
/// hands over the GitHub token they linked to their account
/// ([`crate::central_auth::CentralAuth::linked_github`]), the push and
/// the pull request are made with it, and an Editor who has linked no
/// GitHub account is refused before anything is pushed — so the
/// repository's history is truthful about *which person* landed the
/// change (`wiki.source.editable`). Other forges still land as the
/// deployment, with the deployment's token.
pub struct ForgeLander;

impl Lander for ForgeLander {
    fn identity_for(&self, source: &RepoSource, editor: &str) -> Result<ForgeIdentity, WikiError> {
        identity_for(source, editor)
    }

    fn open_pull_request(
        &self,
        source: &RepoSource,
        landing: &Landing,
        title: &str,
        body: &str,
        identity: &ForgeIdentity,
    ) -> Result<Option<String>, WikiError> {
        open_pull_request(source, landing, title, body, identity)
    }
}

/// The accepting Editor's forge identity for `source`.
///
/// GitHub repositories need the person's linked account: no central
/// issuer, no live credential, or no linked GitHub account is a
/// `Refused` that names the fix. Everything else lands as the
/// deployment. Runs on the blocking pool inside the runtime (the issuer
/// round trip is async).
pub fn identity_for(source: &RepoSource, editor: &str) -> Result<ForgeIdentity, WikiError> {
    let Some((host, owner, repo)) = parse_forge_url(&source.url) else {
        return Ok(ForgeIdentity::deployment(editor));
    };
    if host != "github.com" {
        return Ok(ForgeIdentity::deployment(editor));
    }
    let Some(central) = crate::central_auth::configured() else {
        return Err(WikiError::Refused(
            "this server has no central account issuer, so it cannot land on GitHub as you".into(),
        ));
    };
    let linked = tokio::runtime::Handle::current()
        .block_on(central.linked_github(editor))
        .map_err(|e| {
            let hint = match e {
                crate::central_auth::LinkedTokenError::NotLinked
                | crate::central_auth::LinkedTokenError::NoCredential
                | crate::central_auth::LinkedTokenError::InsufficientScope => format!(
                    " — link GitHub at {}/account, then accept again",
                    central.issuer().trim_end_matches('/')
                ),
                crate::central_auth::LinkedTokenError::Unavailable(_) => String::new(),
            };
            WikiError::Refused(format!("cannot land on GitHub as you: {e}{hint}"))
        })?;
    let login = linked.login.trim().to_owned();
    let display = if login.is_empty() {
        editor.to_owned()
    } else {
        format!("@{login}")
    };
    let committer_name = if login.is_empty() {
        editor.to_owned()
    } else {
        login.clone()
    };
    // GitHub's no-reply address attributes the commit to the account
    // without publishing a mailbox.
    let committer_email = if login.is_empty() {
        format!("{editor}@task.invalid")
    } else {
        format!("{login}@users.noreply.github.com")
    };
    Ok(ForgeIdentity {
        display,
        committer_name,
        committer_email,
        push_url: Some(format!(
            "https://x-access-token:{}@github.com/{owner}/{repo}.git",
            linked.access_token
        )),
        token: Some(linked.access_token),
    })
}

/// How often the loop runs, unless `TASK_WIKI_REPO_SYNC_SECS` says.
const DEFAULT_SYNC_SECS: u64 = 600;

/// The sync interval the deployment asked for.
fn sync_interval() -> Duration {
    std::env::var("TASK_WIKI_REPO_SYNC_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map_or(Duration::from_secs(DEFAULT_SYNC_SECS), Duration::from_secs)
}

/// Keep every repo-sourced wiki of one org current.
///
/// Runs once immediately, then every interval. Each wiki's refresh is
/// a blocking git call, so it runs on the blocking pool; one wiki
/// failing does not stop the others. Logs one line per sync that
/// changed something and one per failure — an unchanged sync is the
/// steady state and rides the span alone.
pub fn spawn_sync_loop(org_slug: String, edits: EditsBackend) {
    tokio::spawn(async move {
        let every = sync_interval();
        loop {
            sync_all(&org_slug, &edits).await;
            tokio::time::sleep(every).await;
        }
    });
}

/// One pass over the org's wikis.
///
/// After a sync that moved the mirror, the Edit lane is asked which of
/// its `Landing` requests the repository now holds
/// (`wiki.source.editable`): those become `Accepted` and close their
/// tracker rows, so "landed" means the repository has it and nothing
/// less.
async fn sync_all(org_slug: &str, edits: &EditsBackend) {
    let backend = edits.wiki().clone();
    for (slug, _) in backend.roots() {
        let Ok(config) = backend.config_of(&slug) else {
            continue;
        };
        let Some(before) = config.source else {
            continue;
        };
        let b = backend.clone();
        let s = slug.clone();
        let result = tokio::task::spawn_blocking(move || b.refresh_source(&s)).await;
        match result {
            Ok(Ok(after)) if after.commit != before.commit => {
                tracing::info!(
                    org.slug = %org_slug,
                    wiki.slug = %slug,
                    wiki.source.commit = %after.commit,
                    "repo-sourced wiki synced"
                );
                let e = edits.clone();
                let s = slug.clone();
                match tokio::task::spawn_blocking(move || e.reconcile_landings(&s)).await {
                    Ok(Ok(landed)) if !landed.is_empty() => tracing::info!(
                        org.slug = %org_slug,
                        wiki.slug = %slug,
                        landed = landed.len(),
                        "edit requests landed upstream"
                    ),
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::warn!(
                        org.slug = %org_slug,
                        wiki.slug = %slug,
                        "reconciling landings failed: {e}"
                    ),
                    Err(e) => tracing::warn!(
                        org.slug = %org_slug,
                        wiki.slug = %slug,
                        "reconciling landings panicked: {e}"
                    ),
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!(
                org.slug = %org_slug,
                wiki.slug = %slug,
                wiki.source.commit = %before.commit,
                "repo-sourced wiki sync failed: {e}"
            ),
            Err(e) => tracing::warn!(
                org.slug = %org_slug,
                wiki.slug = %slug,
                "repo-sourced wiki sync panicked: {e}"
            ),
        }
    }
}

/// `(host, owner, repo)` from a clone URL, or `None` when it has no
/// such shape (a `file://` URL, a bare path).
///
/// Accepts `https://host/owner/repo(.git)`, `ssh://git@host/owner/repo`
/// and `git@host:owner/repo.git`.
#[must_use]
pub fn parse_forge_url(url: &str) -> Option<(String, String, String)> {
    let url = url.trim();
    let rest = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme == "file" {
            return None;
        }
        rest
    } else if let Some((_, rest)) = url.split_once('@').filter(|(u, _)| !u.contains('/')) {
        // scp-like: `git@host:owner/repo.git`
        let (host, path) = rest.split_once(':')?;
        return split_owner_repo(host, path);
    } else {
        return None;
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    let (host, path) = rest.split_once('/')?;
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    split_owner_repo(host, path)
}

fn split_owner_repo(host: &str, path: &str) -> Option<(String, String, String)> {
    let mut parts = path.trim_matches('/').splitn(2, '/');
    let owner = parts.next()?.to_owned();
    let repo = parts
        .next()?
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_owned();
    if host.is_empty() || owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((host.to_ascii_lowercase(), owner, repo))
}

/// Open a pull request for a landing branch on the repository's forge.
///
/// Picks the client by the URL's host: `github.com` with
/// `TASK_GITHUB_TOKEN`; the Forgejo `TASK_FORGEJO_BASE_URL` names with
/// `TASK_FORGEJO_TOKEN`. Returns the pull request's URL, or `Ok(None)`
/// when no client is configured for that host — the branch is pushed
/// and a person opens the request by hand. Must run inside a tokio
/// runtime, on the blocking pool (the forge clients `block_on`).
///
/// Only compiled with the `plugin-git` feature; without it every URL
/// answers `Ok(None)`.
pub fn open_pull_request(
    source: &RepoSource,
    landing: &wiki_live::repo_source::Landing,
    title: &str,
    body: &str,
    identity: &ForgeIdentity,
) -> Result<Option<String>, WikiError> {
    let Some((host, owner, repo)) = parse_forge_url(&source.url) else {
        return Ok(None);
    };
    let base = if source.branch.trim().is_empty() {
        "main".to_owned()
    } else {
        source.branch.trim().to_owned()
    };
    let _ = (&host, &owner, &repo, &base, landing, title, body, identity);
    #[cfg(feature = "plugin-git")]
    {
        use git_proto::reviews::ReviewSurface as _;
        use git_proto::{Forge, NewPullRequest, RepoId};

        let new = || NewPullRequest {
            title: title.to_owned(),
            body: body.to_owned(),
            base: base.clone(),
            head: landing.branch.clone(),
            draft: false,
        };
        if host == "github.com" {
            // The person's own token — `identity_for` refused already
            // when there was none, so the deployment token is only a
            // fallback for a lander that landed as the deployment.
            let Some(token) = identity
                .token
                .clone()
                .or_else(|| env_token("TASK_GITHUB_TOKEN"))
            else {
                return Ok(None);
            };
            let client = git_github::Backend::from_token(token)
                .map_err(|e| WikiError::Backend(format!("github client: {e}")))?;
            let id = RepoId {
                forge: Forge::Github,
                owner: owner.clone(),
                repo: repo.clone(),
            };
            let pr = client
                .create_pull_request(&id, new())
                .map_err(|e| WikiError::Io(format!("github pull request: {e}")))?;
            return Ok(Some(format!(
                "https://github.com/{owner}/{repo}/pull/{}",
                pr.id.0
            )));
        }
        let forgejo_base = std::env::var("TASK_FORGEJO_BASE_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_owned())
            .filter(|s| !s.is_empty());
        if let Some(forgejo_base) = forgejo_base {
            let forgejo_host = forgejo_base
                .split_once("://")
                .map_or(forgejo_base.as_str(), |(_, r)| r)
                .split('/')
                .next()
                .unwrap_or_default()
                .split(':')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if forgejo_host == host {
                let Some(token) = env_token("TASK_FORGEJO_TOKEN") else {
                    return Ok(None);
                };
                let client = git_forgejo::Backend::from_token(forgejo_base.clone(), token)
                    .map_err(|e| WikiError::Backend(format!("forgejo client: {e}")))?;
                let id = RepoId {
                    forge: Forge::Forgejo {
                        base_url: forgejo_base.clone(),
                    },
                    owner: owner.clone(),
                    repo: repo.clone(),
                };
                let pr = client
                    .create_pull_request(&id, new())
                    .map_err(|e| WikiError::Io(format!("forgejo pull request: {e}")))?;
                return Ok(Some(format!(
                    "{forgejo_base}/{owner}/{repo}/pulls/{}",
                    pr.id.0
                )));
            }
        }
    }
    Ok(None)
}

#[cfg(feature = "plugin-git")]
fn env_token(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_urls_parse_and_local_ones_do_not() {
        assert_eq!(
            parse_forge_url("https://github.com/acme/task.git"),
            Some(("github.com".into(), "acme".into(), "task".into()))
        );
        assert_eq!(
            parse_forge_url("git@github.com:acme/task.git"),
            Some(("github.com".into(), "acme".into(), "task".into()))
        );
        assert_eq!(
            parse_forge_url("ssh://git@git.example.org:2222/acme/docs"),
            Some(("git.example.org".into(), "acme".into(), "docs".into()))
        );
        assert_eq!(parse_forge_url("file:///srv/repos/task-docs"), None);
        assert_eq!(parse_forge_url("/srv/repos/task-docs"), None);
    }

    /// A `file://` source has no forge: the branch is pushed and no
    /// request is opened, which the caller reports rather than fails.
    #[tokio::test]
    async fn a_local_repository_yields_no_pull_request() {
        let source = RepoSource {
            url: "file:///tmp/x.git".into(),
            ..Default::default()
        };
        let landing = wiki_live::repo_source::Landing {
            branch: "wiki/edit-1".into(),
            commit: "0".repeat(40),
            pull_request: None,
        };
        assert_eq!(
            open_pull_request(&source, &landing, "t", "b", &ForgeIdentity::deployment("u"))
                .unwrap(),
            None
        );
        // ...and lands as the deployment: no linked account is needed.
        assert_eq!(
            identity_for(&source, "u").unwrap(),
            ForgeIdentity::deployment("u")
        );
    }

    /// A GitHub repository lands only as the accepting person. With no
    /// central issuer configured there is nobody to ask, so the landing
    /// is refused — before anything is pushed.
    #[tokio::test(flavor = "multi_thread")]
    async fn github_without_a_linked_identity_is_refused() {
        let source = RepoSource {
            url: "https://github.com/acme/docs.git".into(),
            ..Default::default()
        };
        // `configured()` reads the env once per process; in the test
        // binary no issuer is set, so the refusal names that.
        let err = tokio::task::block_in_place(|| identity_for(&source, "u")).unwrap_err();
        assert!(
            matches!(&err, WikiError::Refused(m) if m.contains("as you")),
            "{err:?}"
        );
    }

    #[test]
    fn the_interval_defaults() {
        // Only the default is asserted: `set_var` races other tests.
        assert_eq!(sync_interval(), Duration::from_secs(DEFAULT_SYNC_SECS));
    }
}
