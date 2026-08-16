//! Streaming Session checkpoint commit — replaces
//! `files_store::version::checkpoint::checkpoint`'s in-memory
//! `Change::Write { content: Vec<u8> }` shape for `checkpoint_now`'s
//! full-scan write (PR #280 review finding: reading every file into a
//! `Vec<u8>` before handing it to `checkpoint()` peaks RSS at the
//! whole root's size — a 40 GB media root OOMs the shared
//! `task-server`). Each disk file streams straight into the backend via
//! [`crate::content`], which is bounded-memory on both flavors (the CAS
//! backend chunks the reader as it goes; git blobs stream straight into
//! the object database rather than through jj's buffering
//! `GitBackend::write_file` — see that module's doc). A file whose
//! content and mode are unchanged from the checkpoint head is skipped
//! entirely — on the git flavor without even writing an object, since
//! the id is probed first — both to avoid wasted I/O and so
//! `CheckpointInfo::changed_paths` reflects only what actually changed,
//! honoring its own doc contract.
//!
//! This bypasses `checkpoint::checkpoint` (the version-store crate's
//! own convenience helper) and builds the commit directly via jj-lib's
//! `TreeBuilder` + `Transaction`, mirroring that helper's own
//! internals — the ticket's "consume the version-store API as-is"
//! boundary means duplicating this small amount of logic rather than
//! widening that crate's visibility.
//!
//! **Flavor-agnostic** (issue #273): everything here is written against
//! jj-lib's `Backend` trait, so a media root's CAS backend and a software
//! root's stock `GitBackend` produce checkpoints through the same code —
//! which is what makes the chain/history RPC behave identically on both.
//! The one place they differ is copy tracking: git reports
//! `BackendError::Unsupported` from `write_copy`, so software roots use
//! `CopyId::placeholder()` (git's own convention) and their chains fall
//! back to path-following, exactly as `jj` does on a git repo.

use std::collections::BTreeSet;
use std::sync::Arc;

use jj_lib::backend::{
    Backend, BackendError, CommitId, CopyHistory, CopyId, FileId, Tree, TreeId, TreeValue,
};
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use jj_lib::tree_builder::TreeBuilder;
use files_store::version::chain::lookup_dyn;

use crate::certify::{MidHashHook, Settled, StatGuard};
use crate::content::ContentStore;
use crate::error::{Error, Result};
use crate::scan::LiveFile;

/// Everything one capture commit needs. A struct rather than a dozen
/// positional arguments because issues #260 and #273 both added to this
/// call and the next one will too.
pub struct Capture<'a> {
    pub repo: &'a Arc<ReadonlyRepo>,
    pub backend: &'a dyn Backend,
    /// Commit this capture is parented on: the checkpoint head for a
    /// checkpoint, the snapshot branch's tip for an auto-snapshot
    /// (issue #260 — see [`crate::cadence`] on why snapshots branch).
    pub parent_id: CommitId,
    pub base_tree_id: TreeId,
    pub base_tree: &'a Tree,
    pub disk_files: &'a [LiveFile],
    pub base_paths: &'a BTreeSet<RepoPathBuf>,
    pub description: String,
    /// How many times to re-read a file that changed while it was being
    /// hashed before requeueing it (issue #260).
    pub attempts: u32,
    /// Test seam only — see [`crate::certify::MidHashHook`].
    pub hook: Option<MidHashHook>,
}

pub struct CheckpointResult {
    pub repo: Arc<ReadonlyRepo>,
    pub commit_id: CommitId,
    /// Root-relative paths actually written or removed, sorted. Empty
    /// when the live tree was already identical to the previous
    /// checkpoint (a file whose streamed content hashes to the same
    /// `FileId` as the head's is never written).
    pub changed_paths: Vec<String>,
    /// Paths the certifying scan found still being written — the file
    /// changed between the stat taken before hashing it and the one
    /// taken after, on every attempt. They keep their previous
    /// versioned state in this capture and ride into the next one,
    /// sorted (issue #260).
    pub requeued_paths: Vec<String>,
}

/// Streams every file in `disk_files` into the backend, skipping any whose
/// content is unchanged from `head_tree`, removes any `head_paths`
/// entry absent from `disk_files`, and commits the result on top of
/// `parent_id`. v1 detects no renames: a moved file surfaces as a
/// remove+write pair rather than a recorded `CopyHistory` link (ADR
/// 0001: "detection may start simple").
///
/// Ignored files ([`LiveFile::ignored`]) are skipped — unless they are
/// already tracked in `head_paths`, in which case they keep being
/// versioned. An Ignore set decides what *starts* being versioned; it
/// never retroactively deletes history (see [`crate::ignore`]).
pub fn write_checkpoint(capture: Capture<'_>) -> Result<CheckpointResult> {
    pollster::block_on(write_checkpoint_async(capture))
}

/// A fresh copy-history record for a file with no recorded ancestry, or
/// git's placeholder on a backend that doesn't track copies at all.
async fn origin_copy_id(backend: &dyn Backend, path: &RepoPath, salt: Vec<u8>) -> Result<CopyId> {
    let history = CopyHistory {
        current_path: path.to_owned(),
        parents: vec![],
        salt,
    };
    match backend.write_copy(&history).await {
        Ok(id) => Ok(id),
        Err(BackendError::Unsupported(_)) => Ok(CopyId::placeholder()),
        Err(err) => Err(err.into()),
    }
}

async fn write_checkpoint_async(capture: Capture<'_>) -> Result<CheckpointResult> {
    let Capture {
        repo,
        backend,
        parent_id,
        base_tree_id,
        base_tree: head_tree,
        disk_files,
        base_paths: head_paths,
        description,
        attempts,
        hook,
    } = capture;
    let store = repo.store().clone();
    let content = ContentStore::for_repo(repo, backend)?;
    let mut builder = TreeBuilder::new(store.clone(), base_tree_id);
    let mut changed_paths = Vec::new();
    let mut requeued_paths = Vec::new();
    let mut present: BTreeSet<RepoPathBuf> = BTreeSet::new();

    for file in disk_files {
        let repo_path = &file.repo_path;
        if file.ignored && !head_paths.contains(repo_path) {
            // Ignored and untracked: never enters the store. (Ignored but
            // already tracked falls through and keeps being versioned —
            // see this module's doc.)
            continue;
        }
        present.insert(repo_path.clone());
        let existing = lookup_dyn(backend, head_tree, repo_path).await?;

        // A pointer stub (issue #263) is a placeholder, never content:
        // the path keeps its tracked state exactly as the head records
        // it — no write, no changed_paths entry, and (being in
        // `present`) no recorded deletion. Dehydration is invisible to
        // history. A stub at a path the head doesn't track is inert:
        // its bytes must never be ingested, and there is nothing
        // tracked to preserve, so it is simply skipped.
        if file.stub.is_some() {
            continue;
        }

        // The mode to record: the live file's own executable bit where
        // the filesystem has one, otherwise whatever the tree already
        // said (never a silent flip to non-executable — see
        // `scan::is_executable`).
        let executable = if cfg!(unix) {
            file.executable
        } else {
            matches!(
                &existing,
                Some(TreeValue::File {
                    executable: true,
                    ..
                })
            )
        };

        // Certified read (issue #260): a file being written *right now*
        // must never be committed torn. Each attempt stats the file,
        // reads it, and stats again; a file that moved is re-read, and
        // one that is still moving after `attempts` tries is requeued —
        // it keeps its previous versioned state here and rides into the
        // next capture, which is what keeps a running bounce from
        // failing everyone else's checkpoint.
        let mut certified: Option<Option<FileId>> = None;
        for _ in 0..attempts.max(1) {
            let guard = StatGuard::begin(&file.disk_path)?;
            if let Some(hook) = &hook {
                hook(&file.disk_path);
            }

            // Probe the content id before writing anything: an untouched
            // file is then skipped without any object write at all (see
            // `crate::content`). `None` means this backend can't know
            // the id in advance, and the write below establishes it.
            let probed = content.probe(&file.disk_path)?;
            if let (
                Some(new_id),
                Some(TreeValue::File {
                    id: old_id,
                    executable: old_exec,
                    ..
                }),
            ) = (&probed, &existing)
                && old_id == new_id
                && *old_exec == executable
            {
                // Identical to what is already versioned. Whether the
                // file is settled or not is moot: there is nothing to
                // record either way.
                certified = Some(None);
                break;
            }

            let new_id = content.write(repo_path, &file.disk_path, probed).await?;
            match guard.check(&file.disk_path)? {
                Settled::Stable => {
                    certified = Some(Some(new_id));
                    break;
                }
                Settled::Moved => continue,
                Settled::Coarse => {
                    // Timestamps too coarse to prove anything (the NAS
                    // case — see `crate::certify`): prove it by content
                    // instead. Two independent reads that produce the
                    // same id had no write between them.
                    let probed = content.probe(&file.disk_path)?;
                    let again = content.write(repo_path, &file.disk_path, probed).await?;
                    if again == new_id && guard.check(&file.disk_path)? != Settled::Moved {
                        certified = Some(Some(new_id));
                        break;
                    }
                }
            }
        }
        let Some(certified) = certified else {
            requeued_paths.push(repo_path.as_internal_file_string().to_string());
            continue;
        };
        let Some(new_id) = certified else {
            continue;
        };

        let copy_id = match &existing {
            Some(TreeValue::File {
                id: old_id,
                executable: old_exec,
                copy_id,
            }) => {
                if *old_id == new_id && *old_exec == executable {
                    // Unchanged: no builder mutation, no changed_paths
                    // entry — this is what keeps a no-op checkpoint a
                    // true no-op in both the commit tree and the
                    // reported diff.
                    continue;
                }
                copy_id.clone()
            }
            _ => origin_copy_id(backend, repo_path, new_id.as_bytes().to_vec()).await?,
        };
        let value = TreeValue::File {
            id: new_id,
            executable,
            copy_id,
        };
        builder.set(repo_path.clone(), value);
        changed_paths.push(repo_path.as_internal_file_string().to_string());
    }

    for path in head_paths {
        if !present.contains(path) {
            builder.remove(path.clone());
            changed_paths.push(path.as_internal_file_string().to_string());
        }
    }
    changed_paths.sort();
    requeued_paths.sort();

    let new_tree_id = builder
        .write_tree()
        .await
        .map_err(|e| Error::Repo(format!("write_tree: {e}")))?;
    let merged_tree = MergedTree::resolved(store, new_tree_id);

    let mut tx = repo.start_transaction();
    // The commit id comes from the commit we just wrote — never from
    // `view().heads()`, which is an unordered set that on an adopted
    // multi-branch git repo also holds every other branch tip (PR #282
    // review: picking from it could report, and then force the checked-out
    // branch onto, an unrelated branch's head).
    let commit = tx
        .repo_mut()
        .new_commit(vec![parent_id], merged_tree)
        .set_description(description)
        .write()
        .await
        .map_err(|e| Error::Repo(format!("write commit: {e}")))?;
    let commit_id = commit.id().clone();
    let new_repo = tx
        .commit("checkpoint")
        .await
        .map_err(|e| Error::Repo(e.to_string()))?;

    Ok(CheckpointResult {
        repo: new_repo,
        commit_id,
        changed_paths,
        requeued_paths,
    })
}
