//! Bringing a subscribed source onto local disk, and keeping it there.
//!
//! This rides the vault sync engine rather than inventing a second one.
//! A wiki *is* a vault — same primitives, same tree — so
//! `vault_sync_client::{index_local, plan_sync, apply_one}` already
//! knows how to compare two markdown trees and move the difference,
//! over vox to another server just as readily as in process.
//!
//! # Why the plan is filtered rather than applied whole
//!
//! The engine's own policy resolves a conflict by mtime — newer side
//! wins. That is right for *your* vault on two of your machines, and
//! wrong for a subscribed copy, where the spec is explicit that a
//! conflict is "never resolved by recency" and that local work is
//! never overwritten by an upstream update
//! (`wiki.subscribe.refresh`).
//!
//! # Two sides are not enough
//!
//! `plan_sync` compares local against remote, so two differing SHAs
//! are a `Conflict` — it cannot tell "upstream moved and I did not"
//! from "we both moved". For a peer sync that is fine, because the
//! mtime tiebreak resolves either. For a subscription it is not: the
//! first case is ordinary news that must arrive, and the second must
//! never be decided by clock.
//!
//! So a copy records the **base**: the sha of every file as of the
//! last successful refresh. Three sides make the distinction exact.
//!
//! | local vs base | remote vs base | meaning | what happens |
//! |---|---|---|---|
//! | same | changed | upstream's news | pulled |
//! | changed | same | the subscriber's work | kept, never sent |
//! | changed | changed | genuine conflict | kept, reported, never resolved |
//!
//! The base lives outside the copy (`subscribed/.state/`), so the
//! mounted folder stays markdown a person can open
//! (`wiki.local.mount`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vault_proto::VaultSync;
use vault_sync_client::{SyncOp, index_local, plan_sync};

/// Path → sha as of the last successful refresh.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Base(BTreeMap<String, String>);

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("read upstream manifest for `{id}`: {source}")]
    Manifest {
        id: String,
        #[source]
        source: vault_proto::VaultSyncError,
    },
    #[error("sync `{id}`: {source}")]
    Sync {
        id: String,
        #[source]
        source: vault_sync_client::SyncError,
    },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("base state for `{id}`: {source}")]
    Base {
        id: String,
        #[source]
        source: serde_json::Error,
    },
}

/// What a refresh did, and what it deliberately did not do.
///
/// The divergence counts are the point: a subscriber who has edited
/// their copy needs to be told that, not have it quietly reconciled.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refreshed {
    /// Files brought down from upstream.
    pub pulled: usize,
    /// Files already identical.
    pub in_sync: usize,
    /// Pages the subscriber has that upstream does not. Held, never
    /// pushed.
    pub local_only: Vec<String>,
    /// Pages both sides changed. Held, never resolved.
    pub conflicted: Vec<String>,
}

impl Refreshed {
    /// Whether the copy carries local work that upstream has not seen.
    ///
    /// What `wiki.subscribe.local-copy` needs before unsubscribing:
    /// dropping a copy with unpushed changes has to say so and take an
    /// answer, rather than discarding work to tidy up.
    #[must_use]
    pub fn has_local_work(&self) -> bool {
        !self.local_only.is_empty() || !self.conflicted.is_empty()
    }
}

/// Where an org keeps the sources it subscribes to.
///
/// One directory per source, addressed the way a reference addresses
/// it — `<org>/subscribed/<domain>/<slug>/` — so the path a file sits
/// at and the reference that names it cannot drift apart.
#[must_use]
pub fn local_copy_dir(org_root: &Path, domain: &str, slug: &str) -> PathBuf {
    org_root.join("subscribed").join(domain).join(slug)
}

/// Where the base snapshot for one copy lives.
///
/// Outside the copy itself: a subscribed wiki appears in the file sync
/// clients as plain markdown, and a state file sitting in it would be
/// one more thing a person has to know to ignore.
#[must_use]
pub fn base_path(org_root: &Path, domain: &str, slug: &str) -> PathBuf {
    org_root
        .join("subscribed")
        .join(".state")
        .join(domain)
        .join(format!("{slug}.json"))
}

/// Bring a subscribed source's local copy up to date with `upstream`.
///
/// `upstream_id` is the source's id on the far side — its wiki slug.
/// The same call serves an in-process backend and a vox client, since
/// both implement [`VaultSync`].
///
/// t[impl wiki.subscribe.local-copy] — after this returns, the copy
/// resolves with the network down, because everything upstream had is
/// on disk.
/// t[impl wiki.subscribe.refresh] — local work is replayed rather than
/// overwritten: it is simply never touched, and a conflict is reported
/// for a person instead of being decided by clock.
/// t[impl wiki.subscribe.local-authority] — nothing here writes
/// upstream. There is no code path from a refresh to a `put_file`.
///
/// # Errors
///
/// A failure reading the upstream manifest, fetching a file, or
/// writing the local copy.
pub fn refresh<U: VaultSync>(
    upstream: &U,
    upstream_id: &str,
    local_root: &Path,
    base_at: &Path,
) -> Result<Refreshed, MaterializeError> {
    std::fs::create_dir_all(local_root).map_err(|source| MaterializeError::Io {
        path: local_root.display().to_string(),
        source,
    })?;
    let base = load_base(base_at, upstream_id)?;

    let mut manifest = upstream
        .manifest(upstream_id)
        .map_err(|source| MaterializeError::Manifest {
            id: upstream_id.to_owned(),
            source,
        })?;
    // The source's own bookkeeping — its declaration, its Edit
    // Requests, its queues under `_state/` — is not content and is
    // never the subscriber's: a copy that carried the publisher's
    // Editor list or open requests would present someone else's state
    // as its own. Pages only.
    manifest
        .files
        .retain(|f| !is_source_bookkeeping(&f.path));
    let local = index_local(local_root).map_err(|source| MaterializeError::Sync {
        id: upstream_id.to_owned(),
        source,
    })?;

    let mut out = Refreshed::default();
    let mut next = Base::default();
    for entry in &manifest.files {
        next.0.insert(entry.path.clone(), entry.sha256.clone());
    }

    for op in plan_sync(&local, &manifest) {
        match &op {
            SyncOp::Pull { path, remote_sha } => {
                apply(upstream, upstream_id, local_root, &op)?;
                out.pulled += 1;
                next.0.insert(path.clone(), remote_sha.clone());
            }
            SyncOp::InSync { .. } => out.in_sync += 1,
            // A file we have and upstream does not. Either the
            // subscriber wrote it, or upstream deleted it — and
            // without a base those are the same picture. Kept either
            // way: a subscription never deletes the subscriber's
            // files, and `apply_one` would have pushed it upstream,
            // which is exactly what a subscription withholds.
            SyncOp::Push { path, .. } => {
                out.local_only.push(path.clone());
                // Keep whatever the base said, so a later upstream
                // return of this path is still judged against it.
                if let Some(sha) = base.0.get(path) {
                    next.0.insert(path.clone(), sha.clone());
                }
            }
            SyncOp::Conflict {
                path,
                local_sha,
                remote_sha,
                ..
            } => {
                match base.0.get(path.as_str()) {
                    // Local is untouched since the last refresh, so
                    // the difference is upstream's news and arrives.
                    Some(based) if based == local_sha => {
                        apply(upstream, upstream_id, local_root, &SyncOp::Pull {
                            path: path.clone(),
                            remote_sha: remote_sha.clone(),
                        })?;
                        out.pulled += 1;
                        next.0.insert(path.clone(), remote_sha.clone());
                    }
                    // Upstream is untouched: ours is a local edit,
                    // held and never sent.
                    Some(based) if based == remote_sha => {
                        out.local_only.push(path.clone());
                        next.0.insert(path.clone(), based.clone());
                    }
                    // Both moved, or we have no base to judge by.
                    // Never decided here.
                    _ => {
                        out.conflicted.push(path.clone());
                        if let Some(based) = base.0.get(path.as_str()) {
                            next.0.insert(path.clone(), based.clone());
                        }
                    }
                }
            }
        }
    }
    out.local_only.sort();
    out.conflicted.sort();
    save_base(base_at, &next, upstream_id)?;
    Ok(out)
}

fn apply<U: VaultSync>(
    upstream: &U,
    id: &str,
    root: &Path,
    op: &SyncOp,
) -> Result<(), MaterializeError> {
    vault_sync_client::apply_one(upstream, id, root, op).map_err(|source| {
        MaterializeError::Sync {
            id: id.to_owned(),
            source,
        }
    })
}

fn load_base(path: &Path, id: &str) -> Result<Base, MaterializeError> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(Base::default()),
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| MaterializeError::Base {
            id: id.to_owned(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Base::default()),
        Err(source) => Err(MaterializeError::Io {
            path: path.display().to_string(),
            source,
        }),
    }
}

fn save_base(path: &Path, base: &Base, id: &str) -> Result<(), MaterializeError> {
    let io = |source| MaterializeError::Io {
        path: path.display().to_string(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let body = serde_json::to_vec_pretty(base).map_err(|source| MaterializeError::Base {
        id: id.to_owned(),
        source,
    })?;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, &body).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An upstream wiki, served by the real vault backend over a real
    /// directory — the same type the server mounts, so this exercises
    /// the actual engine rather than a stand-in.
    fn upstream_with(files: &[(&str, &str)]) -> (tempfile::TempDir, vault_live::Backend) {
        let tmp = tempfile::tempdir().unwrap();
        for (path, body) in files {
            let full = tmp.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
        let backend = vault_live::Backend::single("music-theory", tmp.path().to_owned()).unwrap();
        (tmp, backend)
    }

    #[test]
    fn a_fresh_subscription_lands_the_whole_source() {
        let (_up, backend) = upstream_with(&[
            ("purpose.md", "# Purpose\n"),
            ("Concepts/Ionian.md", "# Ionian\n"),
        ]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();

        let out = refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();
        assert_eq!(out.pulled, 2);
        assert!(!out.has_local_work());
        assert!(local.path().join("Concepts/Ionian.md").is_file());

        // Idempotent: a second refresh pulls nothing.
        let again = refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();
        assert_eq!(again.pulled, 0);
        assert_eq!(again.in_sync, 2);
    }

    /// t[verify wiki.subscribe.local-copy] — after a refresh the copy
    /// answers on its own. Standing in for "the network is down" by
    /// reading the files directly: nothing about rendering a page
    /// needs upstream once it is here.
    #[test]
    fn the_copy_reads_without_upstream() {
        let (up, backend) = upstream_with(&[("Concepts/Modes.md", "# Modes\nSeven.\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();

        drop(backend);
        drop(up);

        let body = std::fs::read_to_string(local.path().join("Concepts/Modes.md")).unwrap();
        assert!(body.contains("Seven."));
    }

    /// t[verify wiki.subscribe.local-authority] — the subscriber's own
    /// page is kept and is never sent upstream.
    #[test]
    fn a_local_only_page_is_kept_and_never_pushed() {
        let (up, backend) = upstream_with(&[("purpose.md", "# Purpose\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();

        std::fs::write(local.path().join("My Notes.md"), "# Mine\n").unwrap();

        let out = refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();
        assert_eq!(out.local_only, vec!["My Notes.md".to_owned()]);
        assert!(out.has_local_work());
        // Still ours...
        assert!(local.path().join("My Notes.md").is_file());
        // ...and upstream never heard about it.
        assert!(!up.path().join("My Notes.md").exists());
    }

    /// t[verify wiki.subscribe.refresh] — both sides changed one page.
    /// Neither is overwritten and neither wins by clock; the conflict
    /// is reported and the copy stays readable.
    #[test]
    fn a_conflict_is_reported_rather_than_decided() {
        let (up, backend) = upstream_with(&[("Concepts/Ionian.md", "# Ionian\noriginal\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();

        // The subscriber edits; upstream also moves on.
        std::fs::write(
            local.path().join("Concepts/Ionian.md"),
            "# Ionian\nmy edit\n",
        )
        .unwrap();
        std::fs::write(
            up.path().join("Concepts/Ionian.md"),
            "# Ionian\ntheir edit\n",
        )
        .unwrap();

        let out = refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();
        assert_eq!(out.conflicted, vec!["Concepts/Ionian.md".to_owned()]);
        assert_eq!(out.pulled, 0, "an upstream change never overwrites local work");

        // Local work survives verbatim.
        let body = std::fs::read_to_string(local.path().join("Concepts/Ionian.md")).unwrap();
        assert!(body.contains("my edit"));
        // And upstream was not reverted by our stale copy either.
        let theirs = std::fs::read_to_string(up.path().join("Concepts/Ionian.md")).unwrap();
        assert!(theirs.contains("their edit"));
    }

    /// An upstream page that changed with no local edit is ordinary
    /// news and is taken.
    #[test]
    fn an_uncontested_upstream_change_arrives() {
        let (up, backend) = upstream_with(&[("Concepts/Modes.md", "# Modes\nv1\n")]);
        let local = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();

        std::fs::write(up.path().join("Concepts/Modes.md"), "# Modes\nv2\n").unwrap();
        let out = refresh(&backend, "music-theory", local.path(), &base.path().join("b.json")).unwrap();

        assert_eq!(out.pulled, 1);
        let body = std::fs::read_to_string(local.path().join("Concepts/Modes.md")).unwrap();
        assert!(body.contains("v2"));
    }

    #[test]
    fn the_copy_sits_where_a_reference_says_it_does() {
        let org = Path::new("/data/orgs/alice-personal");
        let dir = local_copy_dir(org, "acme.test", "music-theory");
        assert!(dir.ends_with("subscribed/acme.test/music-theory"));
    }
}

/// Refresh one held subscription into its place under `org_root`.
///
/// Wraps [`refresh`] with the two paths a subscription implies, so a
/// caller never has to know where a copy or its base lives — the two
/// must agree, and a caller that computed one and forgot the other
/// would resync from scratch every time.
///
/// # Errors
///
/// As [`refresh`].
/// Whether a path in a source is the source's private bookkeeping
/// rather than a page: anything under `_state/`, at the root or nested.
fn is_source_bookkeeping(path: &str) -> bool {
    let state = wiki_proto::paths::STATE_DIR;
    path == state
        || path.starts_with(&format!("{state}/"))
        || path.contains(&format!("/{state}/"))
}

pub fn refresh_subscription<U: vault_proto::VaultSync>(
    upstream: &U,
    org_root: &Path,
    subscription: &wiki_proto::Subscription,
) -> Result<Refreshed, MaterializeError> {
    let copy = local_copy_dir(org_root, &subscription.domain, &subscription.slug);
    let base = base_path(org_root, &subscription.domain, &subscription.slug);
    refresh(upstream, &subscription.slug, &copy, &base)
}
