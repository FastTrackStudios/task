//! A wiki that mirrors a path inside a git repository
//! (`wiki.source.repo`, `wiki.source.sync`).
//!
//! The repository is the authority and the wiki is an export of one
//! subtree of it. Everything here is a pure function over the `git`
//! binary and two directories:
//!
//! - the **clone**, at `<org>/wikis/.repos/<slug>` — a hidden sibling
//!   of the wikis, so the org's wiki listing (which only accepts
//!   slug-shaped directory names) never mistakes it for a wiki;
//! - the **wiki root**, `<org>/wikis/<slug>`, where the files under
//!   `source.path` are copied to, byte for byte.
//!
//! The export is a mirror, not a merge: a file the repository dropped
//! is deleted from the wiki, and a file the wiki has that the
//! repository never had is left alone. That second half matters
//! because the wiki's own bookkeeping — `_state/` (config, edits,
//! queues) and the scaffold `bootstrap` writes — is not the
//! repository's and must survive every sync. What the mirror wrote
//! is remembered in `_state/repo_mirror.json`, which is how "the
//! repository no longer has it" is told apart from "the repository
//! never had it".
//!
//! Failure leaves the pages exactly as they were. A fetch against a
//! dead URL sets `source.last_error` and changes no page — stale
//! content is served, and marked as stale, rather than served as
//! current or replaced with nothing (`wiki.source.sync`).
//!
//! No `git2`: the server image already carries `git` for the
//! snapshot engine, the software-root tests drive the same binary,
//! and a shell-out is the one way to be sure a clone here behaves
//! like a clone anywhere.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

use wiki_proto::config::RepoSource;
use wiki_proto::error::WikiError;
use wiki_proto::paths;

/// Directory under `<org>/wikis/` that holds every clone. Hidden, and
/// not slug-shaped, so it is never listed as a wiki.
pub const REPOS_DIR: &str = ".repos";

/// The manifest of what the mirror last wrote, inside `_state/`.
const MIRROR_JSON: &str = "repo_mirror.json";

/// What one sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Full sha the wiki now reflects.
    pub commit: String,
    /// Files written or deleted in the wiki root.
    pub changed: u32,
    /// `changed == 0` — the repository had nothing new for us.
    pub unchanged: bool,
}

/// What `land` pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// The branch pushed to `origin`.
    pub branch: String,
    /// Full sha of the commit on it.
    pub commit: String,
    /// URL of a pull request opened for it, when a forge client did.
    /// `land` itself never opens one; the caller may, and records it
    /// here.
    pub pull_request: Option<String>,
}

/// Where a wiki's clone lives: `<wikis_dir>/.repos/<slug>`.
#[must_use]
pub fn clone_dir(wikis_dir: &Path, slug: &str) -> PathBuf {
    wikis_dir.join(REPOS_DIR).join(slug)
}

/// Whether `commit` is reachable from the commit the mirror currently
/// reflects — that is, whether a landing pushed as a branch has since
/// been merged upstream (`wiki.source.editable`). Answered from the
/// clone as it stands; a caller wanting the latest word syncs first.
///
/// `false` for a clone that does not exist yet or a commit git has
/// never heard of: neither is an error, both mean "not landed".
#[must_use]
pub fn contains_commit(wikis_dir: &Path, slug: &str, source: &RepoSource, commit: &str) -> bool {
    if commit.is_empty() || source.commit.is_empty() {
        return false;
    }
    let dir = clone_dir(wikis_dir, slug);
    if !dir.is_dir() {
        return false;
    }
    let _guard = git_lock();
    git(
        &dir,
        &["merge-base", "--is-ancestor", commit, &source.commit],
    )
    .is_ok()
}

/// One process-wide lock around every git operation on any clone.
///
/// A sync and a landing on the same clone both move its HEAD, and the
/// server's periodic sync can coincide with a person's refresh. Serial
/// is correct and cheap; these are seconds apart at most.
fn git_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run `git` in `dir`, returning stdout, or the stderr as the error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Never prompt: a URL that wants a password fails instead of
        // hanging a server thread.
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("running git {args:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if err.is_empty() {
            format!("git {args:?} failed with {}", out.status)
        } else {
            err
        })
    }
}

/// Whether `git` is on this machine's PATH.
#[must_use]
pub fn git_on_path() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Clone if absent, fetch, and resolve the commit `source` follows.
///
/// Returns the clone's directory and the full sha of `origin/<branch>`
/// (or the remote's HEAD when the branch is empty).
fn fetch(wikis_dir: &Path, slug: &str, source: &RepoSource) -> Result<(PathBuf, String), String> {
    let clone = clone_dir(wikis_dir, slug);
    if !clone.join(".git").exists() {
        let parent = clone.parent().unwrap_or(wikis_dir);
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        // `--no-checkout`: the working tree is populated at the commit
        // we choose, not at whatever the remote's HEAD happens to be.
        git(
            parent,
            &[
                "clone",
                "--no-checkout",
                &source.url,
                clone.file_name().and_then(|f| f.to_str()).unwrap_or(slug),
            ],
        )?;
    } else {
        // A URL edit in the config must take effect on the next sync
        // rather than fetching the old remote forever.
        git(&clone, &["remote", "set-url", "origin", &source.url])?;
    }
    git(&clone, &["fetch", "--prune", "origin"])?;
    let target = if source.branch.trim().is_empty() {
        // Ask the remote which branch is its HEAD, every time: a repo
        // that switched from `master` to `main` should be followed.
        git(&clone, &["remote", "set-head", "origin", "--auto"])?;
        "refs/remotes/origin/HEAD".to_owned()
    } else {
        format!("refs/remotes/origin/{}", source.branch.trim())
    };
    let sha = git(
        &clone,
        &["rev-parse", "--verify", &format!("{target}^{{commit}}")],
    )?
    .trim()
    .to_owned();
    Ok((clone, sha))
}

/// Bring the wiki root up to date with its repository.
///
/// Clone if absent, fetch, check out the followed commit, and export
/// `source.path` into `wiki_root`. On success `source.commit` and
/// `source.fetched_at` are set and `source.last_error` cleared; on any
/// failure `source.last_error` says why, the pages are untouched, and
/// the error is returned. The caller persists `source` either way —
/// that is what makes a stale wiki *say* it is stale.
///
/// The slug is the wiki root's directory name, which is also the clone's.
pub fn sync(
    wikis_dir: &Path,
    wiki_root: &Path,
    source: &mut RepoSource,
) -> Result<SyncOutcome, WikiError> {
    let slug = wiki_root
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| WikiError::Backend(format!("{}: no slug", wiki_root.display())))?;
    let _held = git_lock();
    match sync_inner(wikis_dir, wiki_root, slug, source) {
        Ok(outcome) => {
            source.commit = outcome.commit.clone();
            source.fetched_at = chrono::Utc::now().to_rfc3339();
            source.last_error.clear();
            Ok(outcome)
        }
        Err(reason) => {
            source.last_error = reason.clone();
            Err(WikiError::Io(format!("sync {}: {reason}", source.url)))
        }
    }
}

fn sync_inner(
    wikis_dir: &Path,
    wiki_root: &Path,
    slug: &str,
    source: &RepoSource,
) -> Result<SyncOutcome, String> {
    let (clone, sha) = fetch(wikis_dir, slug, source)?;
    // The commit is already reflected: nothing to export. `changed`
    // is still computed below when the commit is new, since two
    // commits can leave `source.path` identical.
    git(&clone, &["checkout", "--force", "--detach", &sha])?;

    let subpath = source.path.trim().trim_matches('/');
    let mut ls = vec!["ls-files", "-z", "--"];
    if !subpath.is_empty() {
        ls.push(subpath);
    }
    let listed = git(&clone, &ls)?;
    let tracked: BTreeSet<String> = listed
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|repo_rel| {
            if subpath.is_empty() {
                repo_rel.to_owned()
            } else {
                repo_rel
                    .strip_prefix(subpath)
                    .and_then(|r| r.strip_prefix('/'))
                    .unwrap_or(repo_rel)
                    .to_owned()
            }
        })
        // Nothing may land in the wiki's own bookkeeping.
        .filter(|rel| {
            !rel.starts_with(&format!("{}/", paths::STATE_DIR)) && rel != paths::STATE_DIR
        })
        .collect();
    if tracked.is_empty() && !subpath.is_empty() && !clone.join(subpath).is_dir() {
        return Err(format!(
            "`{subpath}` is not a directory in the repository at {sha}"
        ));
    }

    let mut changed: u32 = 0;
    let src_base = if subpath.is_empty() {
        clone.clone()
    } else {
        clone.join(subpath)
    };
    for rel in &tracked {
        let from = src_base.join(rel);
        let to = wiki_root.join(rel);
        let bytes = std::fs::read(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        if std::fs::read(&to)
            .map(|have| have == bytes)
            .unwrap_or(false)
        {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&to, bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        changed += 1;
    }

    // Delete what the mirror wrote last time and the repository no
    // longer has. Only that: the wiki's scaffold and state are not
    // the repository's to remove.
    let previous = read_manifest(wiki_root);
    for stale in previous.difference(&tracked) {
        let path = wiki_root.join(stale);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                changed += 1;
                prune_empty_parents(wiki_root, &path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    write_manifest(wiki_root, &tracked)?;

    Ok(SyncOutcome {
        commit: sha,
        changed,
        unchanged: changed == 0,
    })
}

fn manifest_path(wiki_root: &Path) -> PathBuf {
    wiki_root.join(paths::STATE_DIR).join(MIRROR_JSON)
}

fn read_manifest(wiki_root: &Path) -> BTreeSet<String> {
    std::fs::read(manifest_path(wiki_root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_manifest(wiki_root: &Path, files: &BTreeSet<String>) -> Result<(), String> {
    let path = manifest_path(wiki_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(files).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Remove now-empty directories between a deleted file and the root,
/// so a folder the repository dropped does not linger as an empty
/// page group. Stops at the first non-empty one, and never removes
/// the root.
fn prune_empty_parents(wiki_root: &Path, deleted: &Path) {
    let mut dir = deleted.parent();
    while let Some(d) = dir {
        if d == wiki_root || !d.starts_with(wiki_root) {
            break;
        }
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Commit changes onto a new branch in the clone and push it.
///
/// `changes` are repository-relative paths **under `source.path`**
/// (the wiki-relative page path, in other words) with the new content,
/// or `None` to delete. The branch is created from `source.commit` —
/// the commit the wiki reflects, so the diff is exactly what the
/// Editor accepted — or from the followed branch's tip when the wiki
/// has never synced. Author and committer are both `author`, because
/// the repository's history must say who landed it
/// (`wiki.source.editable`).
///
/// Nothing here opens a pull request; see the server's `wiki_repo`
/// module for that, which needs a forge client this crate does not
/// carry.
pub fn land(
    wikis_dir: &Path,
    slug: &str,
    source: &RepoSource,
    branch: &str,
    changes: &[(String, Option<String>)],
    message: &str,
    author: (&str, &str),
) -> Result<Landing, WikiError> {
    let _held = git_lock();
    land_inner(wikis_dir, slug, source, branch, changes, message, author)
        .map_err(|reason| WikiError::Io(format!("land on {}: {reason}", source.url)))
}

fn land_inner(
    wikis_dir: &Path,
    slug: &str,
    source: &RepoSource,
    branch: &str,
    changes: &[(String, Option<String>)],
    message: &str,
    (name, email): (&str, &str),
) -> Result<Landing, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("a landing needs a branch name".into());
    }
    if changes.is_empty() {
        return Err("nothing to land".into());
    }
    let (clone, tip) = fetch(wikis_dir, slug, source)?;
    let base = if source.commit.trim().is_empty() {
        tip
    } else {
        source.commit.trim().to_owned()
    };
    git(&clone, &["checkout", "--force", "-B", branch, &base])?;

    let subpath = source.path.trim().trim_matches('/');
    let base_dir = if subpath.is_empty() {
        clone.clone()
    } else {
        clone.join(subpath)
    };
    for (rel, content) in changes {
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() || rel.split('/').any(|seg| seg == ".." || seg == ".git") {
            return Err(format!("refusing path `{rel}`"));
        }
        let path = base_dir.join(rel);
        match content {
            Some(text) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("{}: {e}", parent.display()))?;
                }
                std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            None => match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            },
        }
    }
    let mut add = vec!["add", "-A", "--"];
    add.push(if subpath.is_empty() { "." } else { subpath });
    git(&clone, &add)?;
    let ident_name = format!("user.name={name}");
    let ident_email = format!("user.email={email}");
    let author_line = format!("{name} <{email}>");
    git(
        &clone,
        &[
            "-c",
            &ident_name,
            "-c",
            &ident_email,
            "commit",
            "--author",
            &author_line,
            "-m",
            message,
        ],
    )?;
    let commit = git(&clone, &["rev-parse", "HEAD"])?.trim().to_owned();
    git(&clone, &["push", "origin", &format!("{branch}:{branch}")])?;
    // Leave the clone detached at the base again so a later sync
    // finds the shape it expects and the branch is not left checked
    // out with the landing's tree over the mirror's.
    let _ = git(&clone, &["checkout", "--force", "--detach", &base]);
    Ok(Landing {
        branch: branch.to_owned(),
        commit,
        pull_request: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `git` with a fixed identity, asserting success.
    fn g(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.email=test@example.com", "-c", "user.name=Test"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A bare remote and a working clone of it with `docs/` committed
    /// on `main`. Returns `(bare, work)`.
    fn upstream(dir: &Path) -> (PathBuf, PathBuf) {
        let bare = dir.join("remote.git");
        std::fs::create_dir_all(&bare).unwrap();
        g(&bare, &["init", "--bare", "--initial-branch=main"]);
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        g(&work, &["init", "--initial-branch=main"]);
        std::fs::write(work.join("README.md"), "# not docs\n").unwrap();
        std::fs::create_dir_all(work.join("docs/guide")).unwrap();
        std::fs::write(work.join("docs/index.md"), "# Docs\n").unwrap();
        std::fs::write(work.join("docs/guide/setup.md"), "# Setup\n\nStep one.\n").unwrap();
        std::fs::write(work.join("docs/old.md"), "# Old\n").unwrap();
        g(&work, &["add", "-A"]);
        g(&work, &["commit", "-m", "docs"]);
        g(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        g(&work, &["push", "-u", "origin", "main"]);
        (bare, work)
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn source(bare: &Path) -> RepoSource {
        RepoSource {
            url: file_url(bare),
            branch: "main".into(),
            path: "docs".into(),
            ..Default::default()
        }
    }

    fn is_sha(s: &str) -> bool {
        s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// The clone sits where no wiki listing looks: hidden, and not a
    /// slug, so a directory scan that accepts only slug-shaped names
    /// never lists it.
    #[test]
    fn the_clone_is_a_hidden_sibling_and_not_slug_shaped() {
        let d = clone_dir(Path::new("/org/wikis"), "docs");
        assert_eq!(d, PathBuf::from("/org/wikis/.repos/docs"));
        let parent = d.parent().unwrap().file_name().unwrap().to_str().unwrap();
        assert!(parent.starts_with('.'));
        assert_ne!(
            wiki_proto::config::slugify(parent),
            parent,
            "`.repos` must not pass as a slug"
        );
    }

    /// t[verify wiki.source.repo] — the first sync mirrors the subpath
    /// and nothing outside it, and records the commit it reflects.
    #[test]
    fn the_first_sync_mirrors_the_subpath_and_records_the_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);

        let out = sync(&wikis, &root, &mut src).unwrap();
        assert!(is_sha(&out.commit), "{out:?}");
        assert_eq!(out.commit, g(&work, &["rev-parse", "HEAD"]).trim());
        assert_eq!(out.changed, 3);
        assert!(!out.unchanged);
        assert_eq!(src.commit, out.commit);
        assert!(!src.fetched_at.is_empty());
        assert!(src.last_error.is_empty());

        assert_eq!(
            std::fs::read_to_string(root.join("index.md")).unwrap(),
            "# Docs\n"
        );
        assert!(root.join("guide/setup.md").is_file());
        assert!(
            !root.join("README.md").exists(),
            "outside `docs/` is not mirrored"
        );
        assert!(
            !root.join("docs").exists(),
            "the subpath is flattened into the root"
        );
        assert!(clone_dir(&wikis, "docs").join(".git").exists());

        // Again, with nothing new: no change, same commit.
        let again = sync(&wikis, &root, &mut src).unwrap();
        assert!(again.unchanged);
        assert_eq!(again.commit, out.commit);
    }

    /// t[verify wiki.source.sync] — a commit upstream becomes wiki
    /// content on the next sync without anyone re-importing: changed
    /// pages update, a dropped page goes, and the wiki's own `_state/`
    /// is never the repository's to touch.
    #[test]
    fn a_later_commit_updates_pages_deletes_dropped_ones_and_spares_state() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        let first = sync(&wikis, &root, &mut src).unwrap();

        // The wiki's own bookkeeping and scaffold, not from the repo.
        std::fs::create_dir_all(root.join("_state")).unwrap();
        std::fs::write(root.join("_state/wiki.json"), r#"{"slug":"docs"}"#).unwrap();
        std::fs::write(root.join("purpose.md"), "# Docs wiki\n").unwrap();

        std::fs::write(work.join("docs/index.md"), "# Docs v2\n").unwrap();
        std::fs::write(work.join("docs/guide/deploy.md"), "# Deploy\n").unwrap();
        std::fs::remove_file(work.join("docs/old.md")).unwrap();
        g(&work, &["add", "-A"]);
        g(&work, &["commit", "-m", "v2"]);
        g(&work, &["push", "origin", "main"]);

        let second = sync(&wikis, &root, &mut src).unwrap();
        assert_ne!(second.commit, first.commit);
        assert_eq!(second.changed, 3, "one edit, one add, one delete");
        assert_eq!(
            std::fs::read_to_string(root.join("index.md")).unwrap(),
            "# Docs v2\n"
        );
        assert!(root.join("guide/deploy.md").is_file());
        assert!(!root.join("old.md").exists(), "a dropped page is gone");
        assert_eq!(
            std::fs::read_to_string(root.join("_state/wiki.json")).unwrap(),
            r#"{"slug":"docs"}"#,
            "_state/ survives a sync"
        );
        assert!(
            root.join("purpose.md").is_file(),
            "the wiki's own scaffold survives"
        );
    }

    /// t[verify wiki.source.sync] — a fetch that fails says so on the
    /// source and serves what it had, rather than serving nothing.
    #[test]
    fn a_broken_url_sets_last_error_and_leaves_pages_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, _work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        let first = sync(&wikis, &root, &mut src).unwrap();

        src.url = file_url(&tmp.path().join("nowhere.git"));
        let err = sync(&wikis, &root, &mut src).unwrap_err();
        assert!(matches!(err, WikiError::Io(_)), "{err:?}");
        assert!(
            !src.last_error.is_empty(),
            "the failure is recorded on the source"
        );
        assert_eq!(
            src.commit, first.commit,
            "the reflected commit is unchanged"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("index.md")).unwrap(),
            "# Docs\n"
        );
        assert!(root.join("guide/setup.md").is_file());

        // A repo that never synced fails the same way, with nothing
        // written.
        let root2 = wikis.join("other");
        std::fs::create_dir_all(&root2).unwrap();
        let mut bad = RepoSource {
            url: file_url(&tmp.path().join("nowhere.git")),
            ..Default::default()
        };
        assert!(sync(&wikis, &root2, &mut bad).is_err());
        assert!(!bad.last_error.is_empty());
        assert!(bad.commit.is_empty());
        assert!(
            std::fs::read_dir(&root2).unwrap().next().is_none(),
            "nothing written"
        );
    }

    /// An empty branch follows the remote's HEAD, and an empty path is
    /// the repository root.
    #[test]
    fn empty_branch_and_path_mean_remote_head_and_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("all");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = RepoSource {
            url: file_url(&bare),
            ..Default::default()
        };
        let out = sync(&wikis, &root, &mut src).unwrap();
        assert_eq!(out.commit, g(&work, &["rev-parse", "HEAD"]).trim());
        assert!(root.join("README.md").is_file());
        assert!(root.join("docs/index.md").is_file());
    }

    /// t[verify wiki.source.editable] — landing pushes a branch whose
    /// commit the remote holds, authored by the person given, with the
    /// change under the wiki's subpath.
    #[test]
    fn land_pushes_a_branch_with_the_given_author() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, _work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        sync(&wikis, &root, &mut src).unwrap();

        let landing = land(
            &wikis,
            "docs",
            &src,
            "wiki/edit-42",
            &[
                (
                    "guide/setup.md".to_owned(),
                    Some("# Setup\n\nStep one, clearer.\n".to_owned()),
                ),
                ("old.md".to_owned(), None),
            ],
            "Clarify setup; drop old page",
            ("Alice Owner", "alice@acme.test"),
        )
        .unwrap();
        assert_eq!(landing.branch, "wiki/edit-42");
        assert!(is_sha(&landing.commit));
        assert!(landing.pull_request.is_none());

        assert_eq!(
            g(&bare, &["rev-parse", "refs/heads/wiki/edit-42"]).trim(),
            landing.commit,
            "the remote branch is at the landed commit"
        );
        let author = g(&bare, &["log", "-1", "--format=%an <%ae>", &landing.commit]);
        assert_eq!(author.trim(), "Alice Owner <alice@acme.test>");
        let files = g(
            &bare,
            &["show", "--name-status", "--format=", &landing.commit],
        );
        assert!(files.contains("M\tdocs/guide/setup.md"), "{files}");
        assert!(files.contains("D\tdocs/old.md"), "{files}");
        // `main` did not move: landing goes to a branch for review.
        assert_eq!(
            g(&bare, &["rev-parse", "refs/heads/main"]).trim(),
            src.commit
        );
        // And the mirror is untouched until the branch merges and a
        // sync runs.
        assert_eq!(
            std::fs::read_to_string(root.join("guide/setup.md")).unwrap(),
            "# Setup\n\nStep one.\n"
        );
    }
}
