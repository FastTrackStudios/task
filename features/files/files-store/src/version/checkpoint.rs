//! Files' "Session checkpoint" concept (`apps/task/CONTEXT.md`), expressed
//! here at the lowest level: one jj commit on top of a parent, built from an
//! explicit list of [`Change`]s. This is *not* the eventual checkpoint
//! service (that watches a live tree and debounces); it's the seam this
//! ticket's acceptance criteria exercise directly, matching the spec's
//! Testing Decisions ("secondary harness: the version-store backend trait
//! directly ... recorded renames").
//!
//! Renames are recorded fact, not inferred: a caller who knows a save
//! renamed `old/name` to `new/name` says so via [`Change::Rename`], and this
//! module writes the `CopyHistory` link at that moment (ADR 0001: "storage
//! of copy records ships early because retrofitting it after history exists
//! is a bad migration").

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use jj_lib::backend::{CommitId, TreeValue};
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use jj_lib::tree_builder::TreeBuilder;

use crate::version::backend::VersionStoreBackend;
use crate::version::chain::lookup_dyn;
use crate::version::error::{Error, Result};

/// One recorded change to apply in a checkpoint commit.
#[derive(Debug, Clone)]
pub enum Change {
    /// Write (create or overwrite) a file's content. If the path already
    /// has a file, its copy-history lineage carries forward unchanged
    /// (`CopyHistory` doc: "unchanged when a file is modified"); otherwise
    /// a fresh origin is recorded.
    Write { path: RepoPathBuf, content: Vec<u8> },
    /// Delete a path.
    Remove { path: RepoPathBuf },
    /// Move `from` to `to`, optionally also changing its content in the
    /// same checkpoint. Writes a `CopyHistory` record linking `to` back to
    /// `from`'s lineage, so `chain::version_chain` and
    /// `Backend::get_copy_records` both see it as recorded fact.
    Rename {
        from: RepoPathBuf,
        to: RepoPathBuf,
        new_content: Option<Vec<u8>>,
    },
}

fn backend_of(repo: &Arc<ReadonlyRepo>) -> Result<&VersionStoreBackend> {
    repo.store()
        .backend_impl::<VersionStoreBackend>()
        .ok_or_else(|| Error::Repo("repo's store is not a VersionStoreBackend".into()))
}

/// Resolves `path` against the checkpoint's *accumulated* state — every
/// earlier `Change` in this same call, not just the parent commit's tree —
/// so a `Write` followed by a `Rename` of that same path (or a `Rename`
/// followed by a further `Write` to its destination) sees what the prior
/// change actually produced, not what was there before the checkpoint
/// started. `overrides` mirrors exactly what's been staged on the
/// `TreeBuilder` so far: `Some(value)` for a set path, `None` for a
/// removed one, absent for anything this checkpoint hasn't touched yet
/// (which falls through to the parent's base tree).
async fn resolve(
    backend: &VersionStoreBackend,
    base_tree: &jj_lib::backend::Tree,
    overrides: &BTreeMap<RepoPathBuf, Option<TreeValue>>,
    path: &RepoPath,
) -> Result<Option<TreeValue>> {
    if let Some(value) = overrides.get(path) {
        return Ok(value.clone());
    }
    lookup_dyn(backend, base_tree, path).await
}

/// Write one checkpoint commit on top of `parent_id`, applying `changes` in
/// order, and publish it (a real jj transaction/operation — this is what
/// makes op-log semantics, including divergence, apply through this
/// backend exactly as they would through any other).
pub async fn checkpoint(
    repo: &Arc<ReadonlyRepo>,
    parent_id: CommitId,
    changes: Vec<Change>,
    description: impl Into<String>,
) -> Result<Arc<ReadonlyRepo>> {
    let store = repo.store().clone();
    let backend = backend_of(repo)?;

    let parent_commit = store
        .get_commit_async(&parent_id)
        .await
        .map_err(Error::from)?;
    let base_tree_id = parent_commit
        .tree()
        .tree_ids()
        .as_resolved()
        .cloned()
        .ok_or_else(|| {
            Error::Object("checkpoint onto a conflicted tree is unsupported (v1)".into())
        })?;
    let base_tree = backend.tree(&base_tree_id).await?;

    let mut builder = TreeBuilder::new(store.clone(), base_tree_id);
    let mut overrides: BTreeMap<RepoPathBuf, Option<TreeValue>> = BTreeMap::new();

    for change in changes {
        match change {
            Change::Write { path, content } => {
                let file_id = backend
                    .chunks()
                    .write_stream(Cursor::new(content))
                    .await
                    .map_err(Error::from)?;
                let copy_id = match resolve(backend, &base_tree, &overrides, &path).await? {
                    Some(TreeValue::File { copy_id, .. }) => copy_id,
                    _ => {
                        backend
                            .write_origin_copy(&path, file_id.as_bytes().to_vec())
                            .await?
                    }
                };
                let value = TreeValue::File {
                    id: jj_lib::backend::FileId::from_bytes(file_id.as_bytes()),
                    executable: false,
                    copy_id,
                };
                overrides.insert(path.clone(), Some(value.clone()));
                builder.set(path, value);
            }
            Change::Remove { path } => {
                overrides.insert(path.clone(), None);
                builder.remove(path);
            }
            Change::Rename {
                from,
                to,
                new_content,
            } => {
                let Some(TreeValue::File {
                    id: old_file_id,
                    executable,
                    copy_id: old_copy_id,
                }) = resolve(backend, &base_tree, &overrides, &from).await?
                else {
                    return Err(Error::Object(format!(
                        "rename source {from:?} is not a file in the checkpoint's accumulated state"
                    )));
                };
                let new_copy_id = backend.write_copy_from(&to, old_copy_id).await?;
                let new_file_id = match new_content {
                    Some(content) => jj_lib::backend::FileId::from_bytes(
                        backend
                            .chunks()
                            .write_stream(Cursor::new(content))
                            .await
                            .map_err(Error::from)?
                            .as_bytes(),
                    ),
                    None => old_file_id,
                };
                let value = TreeValue::File {
                    id: new_file_id,
                    executable,
                    copy_id: new_copy_id,
                };
                overrides.insert(from.clone(), None);
                overrides.insert(to.clone(), Some(value.clone()));
                builder.remove(from);
                builder.set(to, value);
            }
        }
    }

    let new_tree_id = builder.write_tree().await.map_err(Error::from)?;
    let merged_tree = MergedTree::resolved(store, new_tree_id);

    let mut tx = repo.start_transaction();
    let description = description.into();
    tx.repo_mut()
        .new_commit(vec![parent_id], merged_tree)
        .set_description(description)
        .write()
        .await
        .map_err(Error::from)?;
    tx.commit("checkpoint")
        .await
        .map_err(|e| Error::Repo(e.to_string()))
}
