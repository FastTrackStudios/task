//! Derives per-file version chains from the commit DAG (ADR 0001: "per-file
//! chains are derived", never a second source of truth) and implements the
//! DAG-range copy-record walk that backs
//! [`jj_lib::backend::Backend::get_copy_records`].
//!
//! [`version_chain`] walks first-parent: Files' own checkpoint cadence
//! never creates merge commits (only jj's automatic op-log reconciliation
//! does — see `checkpoint.rs`'s divergence demo), but an adopted software
//! root's git history is full of them, and a chain that stopped at the
//! first merge would report a decade-old file as having one version. A
//! divergent file's history is still presented as sibling versions, not
//! walked through as one chain.

use std::collections::BTreeSet;

use jj_lib::backend::{
    Backend, BackendError, CommitId, CopyHistory, CopyId, CopyRecord, Tree, TreeValue,
};
use jj_lib::repo_path::{RepoPath, RepoPathBuf};

use crate::backend::VersionStoreBackend;
use crate::error::{Error, Result};

/// One entry in a file's version chain: the commit that produced this saved
/// state, the path the file lived at in that commit (chains follow
/// renames), and its content address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionEntry {
    pub commit_id: CommitId,
    pub path: RepoPathBuf,
    pub file_id: jj_lib::backend::FileId,
    /// Set when this entry is the commit where the file arrived at `path`
    /// via a recorded rename (as opposed to a plain content edit).
    pub renamed_from: Option<RepoPathBuf>,
}

async fn resolved_tree_id(commit: &jj_lib::backend::Commit) -> Result<jj_lib::backend::TreeId> {
    commit.root_tree.clone().into_resolved().map_err(|_| {
        Error::Object("conflicted trees are not supported by chain derivation (v1)".into())
    })
}

/// Look up `path` inside `tree`, descending through subtrees as needed.
///
/// Written against the `Backend` trait rather than this crate's own type,
/// so a software File Root's stock `GitBackend` (ADR 0001's other Root
/// flavor, issue #273) is walked by exactly this descent — and public so
/// that Files' own checkpoint writer uses it too rather than keeping a
/// second copy in step by hand (PR #282 review).
pub async fn lookup_dyn(
    backend: &dyn Backend,
    tree: &Tree,
    path: &RepoPath,
) -> Result<Option<TreeValue>> {
    let Some((dir, basename)) = path.split() else {
        return Ok(None);
    };
    if dir.as_internal_file_string().is_empty() {
        return Ok(tree.value(basename).cloned());
    }
    // Descend one component at a time from the root, carrying the path so
    // far — backends that key trees by path (unlike this crate's own, which
    // is purely content-addressed) get the location they expect.
    let mut current = tree.clone();
    let mut prefix = RepoPathBuf::root();
    for component in dir.components() {
        prefix = prefix.join(component);
        match current.value(component) {
            Some(TreeValue::Tree(id)) => {
                current = backend.read_tree(&prefix, id).await?;
            }
            _ => return Ok(None),
        }
    }
    Ok(current.value(basename).cloned())
}

/// Read `copy_id`'s history, or `None` when the backend doesn't track
/// copies at all. Stock git is such a backend (`BackendError::Unsupported`
/// from every copy method), so on a software File Root a file that isn't in
/// its parent commit at the same path is simply the file's origin — there is
/// no recorded-rename link to follow. On the media backend, which does
/// record copies (ADR 0001: "recorded renames in backend v1"), this always
/// returns `Some`.
async fn copy_history_opt(backend: &dyn Backend, copy_id: &CopyId) -> Result<Option<CopyHistory>> {
    match backend.read_copy(copy_id).await {
        Ok(history) => Ok(Some(history)),
        Err(BackendError::Unsupported(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Derive the version chain for `path` as seen from `head`, newest first.
///
/// Written against the `Backend` trait, not this crate's own backend type:
/// both Root flavors (ADR 0001 — media on the CAS backend, software on
/// stock colocated git) derive chains through this one walk, which is what
/// makes the Files chain/history RPC behave identically on either.
pub async fn version_chain(
    backend: &dyn Backend,
    head: &CommitId,
    path: &RepoPath,
) -> Result<Vec<VersionEntry>> {
    let mut entries = Vec::new();
    let mut commit_id = head.clone();
    let mut tracked_path = path.to_owned();

    loop {
        let commit = backend.read_commit(&commit_id).await?;
        let tree_id = resolved_tree_id(&commit).await?;
        let tree = backend.read_tree(RepoPath::root(), &tree_id).await?;
        let Some(TreeValue::File { id, copy_id, .. }) =
            lookup_dyn(backend, &tree, &tracked_path).await?
        else {
            break;
        };

        let Some(parent_id) = commit.parents.first().cloned() else {
            // The root commit: this is the file's earliest known state.
            entries.push(VersionEntry {
                commit_id: commit_id.clone(),
                path: tracked_path.clone(),
                file_id: id,
                renamed_from: None,
            });
            break;
        };
        // Merges are followed along their FIRST parent rather than stopping
        // the walk (PR #282 review): Files' own cadence never merges, but an
        // adopted git repo's history is full of them, and truncating there
        // would report "this file has one version" for a file with years of
        // history. First-parent is git's own `--first-parent` convention:
        // the mainline this branch's saves actually happened on.
        let parent_commit = backend.read_commit(&parent_id).await?;
        let parent_tree_id = resolved_tree_id(&parent_commit).await?;
        let parent_tree = backend.read_tree(RepoPath::root(), &parent_tree_id).await?;
        let parent_value = lookup_dyn(backend, &parent_tree, &tracked_path).await?;

        let (is_new_state, next_path, renamed_from) = match &parent_value {
            Some(TreeValue::File {
                id: parent_id_at_path,
                copy_id: parent_copy_id,
                ..
            }) if *parent_copy_id == copy_id => {
                // Same lineage at the same path: a new saved state only if
                // the content actually changed.
                (*parent_id_at_path != id, tracked_path.clone(), None)
            }
            _ => {
                // Not present with the same lineage at this path in the
                // parent — follow the recorded copy history to see whether
                // this is a rename rather than a fresh origin. A backend
                // with no copy tracking at all (stock git) reports no
                // ancestry, so the file is treated as born here.
                let history = copy_history_opt(backend, &copy_id).await?;
                match history.as_ref().and_then(|h| h.parents.first()) {
                    Some(source_copy_id) => {
                        let source_history = backend
                            .read_copy(source_copy_id)
                            .await
                            .map_err(Error::from)?;
                        let source_path = source_history.current_path.clone();
                        (true, source_path.clone(), Some(source_path))
                    }
                    None => {
                        // No copy ancestry at all: this commit is where the
                        // file was born.
                        entries.push(VersionEntry {
                            commit_id: commit_id.clone(),
                            path: tracked_path.clone(),
                            file_id: id,
                            renamed_from: None,
                        });
                        return Ok(entries);
                    }
                }
            }
        };

        if is_new_state {
            entries.push(VersionEntry {
                commit_id: commit_id.clone(),
                path: tracked_path.clone(),
                file_id: id,
                renamed_from,
            });
        }
        commit_id = parent_id;
        tracked_path = next_path;
    }

    Ok(entries)
}

/// A flat (path, old value, new value) tree diff, recursing into changed
/// subtrees. Used only by [`copy_records_between`] — chain derivation above
/// walks path-first instead, since it already knows which path it's
/// following.
async fn diff_tree<'a>(
    backend: &'a VersionStoreBackend,
    prefix: &'a RepoPath,
    old: Option<&'a Tree>,
    new: Option<&'a Tree>,
    out: &mut Vec<(RepoPathBuf, Option<TreeValue>, Option<TreeValue>)>,
) -> Result<()> {
    let empty = Tree::default();
    let old_tree = old.unwrap_or(&empty);
    let new_tree = new.unwrap_or(&empty);

    let names: BTreeSet<_> = old_tree.names().chain(new_tree.names()).collect();
    for name in names {
        let old_value = old_tree.value(name);
        let new_value = new_tree.value(name);
        if old_value == new_value {
            continue;
        }
        let path = prefix.join(name);
        match (old_value, new_value) {
            (Some(TreeValue::Tree(oid)), Some(TreeValue::Tree(nid))) if oid != nid => {
                let old_sub = backend.tree(oid).await?;
                let new_sub = backend.tree(nid).await?;
                Box::pin(diff_tree(
                    backend,
                    &path,
                    Some(&old_sub),
                    Some(&new_sub),
                    out,
                ))
                .await?;
            }
            (Some(TreeValue::Tree(_)), Some(TreeValue::Tree(_))) => {}
            (Some(TreeValue::Tree(oid)), other) => {
                let old_sub = backend.tree(oid).await?;
                Box::pin(diff_tree(backend, &path, Some(&old_sub), None, out)).await?;
                if let Some(value) = other {
                    out.push((path, None, Some(value.clone())));
                }
            }
            (other, Some(TreeValue::Tree(nid))) => {
                if let Some(value) = other {
                    out.push((path.clone(), Some(value.clone()), None));
                }
                let new_sub = backend.tree(nid).await?;
                Box::pin(diff_tree(backend, &path, None, Some(&new_sub), out)).await?;
            }
            (old_leaf, new_leaf) => {
                out.push((path, old_leaf.cloned(), new_leaf.cloned()));
            }
        }
    }
    Ok(())
}

/// Ancestor commit ids of `head`, reachable by walking `parents` (assumes a
/// backend-local, not-too-deep history — fine at Files' checkpoint cadence
/// per ADR 0001; a path→commits cache is future work if this ever measures
/// slow).
async fn ancestors(backend: &VersionStoreBackend, head: &CommitId) -> Result<Vec<CommitId>> {
    let mut seen = vec![head.clone()];
    let mut frontier = vec![head.clone()];
    while let Some(id) = frontier.pop() {
        let commit = backend.commit(&id).await?;
        for parent in commit.parents {
            if !seen.contains(&parent) {
                seen.push(parent.clone());
                frontier.push(parent);
            }
        }
    }
    Ok(seen)
}

/// Implements `Backend::get_copy_records`: every recorded copy/rename event
/// for commits in the dag range `root..head`, optionally restricted to
/// `paths` (matched against the copy's target).
pub async fn copy_records_between(
    backend: &VersionStoreBackend,
    paths: Option<&[RepoPathBuf]>,
    root: &CommitId,
    head: &CommitId,
) -> Result<Vec<CopyRecord>> {
    let excluded = ancestors(backend, root).await?;
    let mut range = Vec::new();
    let mut seen = vec![head.clone()];
    let mut frontier = vec![head.clone()];
    while let Some(id) = frontier.pop() {
        if excluded.contains(&id) {
            continue;
        }
        let commit = backend.commit(&id).await?;
        range.push((id.clone(), commit.clone()));
        for parent in commit.parents {
            if !seen.contains(&parent) {
                seen.push(parent.clone());
                frontier.push(parent);
            }
        }
    }

    let mut records = Vec::new();
    for (commit_id, commit) in &range {
        for parent_id in &commit.parents {
            let parent_commit = backend.commit(parent_id).await?;
            let tree_id = resolved_tree_id(commit).await?;
            let parent_tree_id = resolved_tree_id(&parent_commit).await?;
            let tree = backend.tree(&tree_id).await?;
            let parent_tree = backend.tree(&parent_tree_id).await?;

            let mut diffs = Vec::new();
            diff_tree(
                backend,
                RepoPath::root(),
                Some(&parent_tree),
                Some(&tree),
                &mut diffs,
            )
            .await?;

            for (path, old_value, new_value) in diffs {
                let Some(TreeValue::File { copy_id, .. }) = &new_value else {
                    continue;
                };
                if let Some(paths) = paths {
                    if !paths.contains(&path) {
                        continue;
                    }
                }
                let unchanged_lineage = matches!(
                    &old_value,
                    Some(TreeValue::File { copy_id: old_copy_id, .. }) if old_copy_id == copy_id
                );
                if unchanged_lineage {
                    continue;
                }
                let history = backend.copy_history(copy_id).await?;
                for source_copy_id in &history.parents {
                    let source_history = backend.copy_history(source_copy_id).await?;
                    let source_path = source_history.current_path.clone();
                    // The source must actually be a file in the parent
                    // commit's tree — anything else (absent, a directory)
                    // means there is no real file content to report as
                    // "copied from" in this commit, and substituting the
                    // *target's* new file id would fabricate content that
                    // never existed at `source_path`. Skip the record
                    // rather than inventing one.
                    let Some(TreeValue::File {
                        id: source_file, ..
                    }) = lookup_dyn(backend, &parent_tree, &source_path).await?
                    else {
                        continue;
                    };
                    records.push(CopyRecord {
                        target: path.clone(),
                        target_commit: commit_id.clone(),
                        source: source_path,
                        source_file,
                        source_commit: parent_id.clone(),
                    });
                }
            }
        }
    }

    Ok(records)
}

#[cfg(test)]
mod tests {
    use jj_lib::backend::TreeValue;
    use jj_lib::merged_tree::MergedTree;
    use jj_lib::repo::Repo as _;
    use jj_lib::repo_path::RepoPathBuf;
    use jj_lib::tree_builder::TreeBuilder;

    use super::copy_records_between;
    use super::lookup_dyn;
    use crate::backend::VersionStoreBackend;
    use crate::checkpoint::{Change, checkpoint};
    use crate::repo::init_repo;

    fn path(s: &str) -> RepoPathBuf {
        RepoPathBuf::from_internal_string(s).unwrap()
    }

    /// Regression test for a defect where a copy-history record naming a
    /// source path that's absent from the immediate parent tree (e.g. the
    /// source was deleted in an intermediate commit) caused
    /// `copy_records_between` to fabricate `source_file` from the
    /// *target's own* new content. `checkpoint::checkpoint`'s own
    /// accumulated-state validation can't produce this — `Change::Rename`
    /// always requires the source to currently exist — so this test builds
    /// the scenario directly against the `Backend` trait, the way any other
    /// caller writing commits straight through `write_tree`/`write_commit`
    /// legitimately could.
    #[tokio::test]
    async fn copy_record_is_skipped_not_fabricated_when_source_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(dir.path().join("repo").as_path()).await.unwrap();
        let root_id = repo.store().root_commit_id().clone();
        let a = path("a");
        let c = path("c");

        // commit1: create `a`.
        let repo = checkpoint(
            &repo,
            root_id,
            vec![Change::Write {
                path: a.clone(),
                content: b"hello".to_vec(),
            }],
            "create a",
        )
        .await
        .unwrap();
        let commit1 = repo.view().heads().iter().next().unwrap().clone();

        let backend = repo.store().backend_impl::<VersionStoreBackend>().unwrap();
        let commit1_tree_id = repo
            .store()
            .get_commit_async(&commit1)
            .await
            .unwrap()
            .tree()
            .tree_ids()
            .as_resolved()
            .cloned()
            .unwrap();
        let commit1_tree = backend.tree(&commit1_tree_id).await.unwrap();
        let Some(TreeValue::File {
            copy_id: a_copy_id, ..
        }) = lookup_dyn(backend, &commit1_tree, &a).await.unwrap()
        else {
            panic!("a should be a file in commit1's tree");
        };

        // commit2: remove `a` (the source genuinely stops existing here).
        let repo = checkpoint(
            &repo,
            commit1.clone(),
            vec![Change::Remove { path: a.clone() }],
            "remove a",
        )
        .await
        .unwrap();
        let commit2 = repo.view().heads().iter().next().unwrap().clone();

        // commit3: planted directly through the `Backend` trait (bypassing
        // `checkpoint`'s own validation) — a file at `c` whose copy history
        // claims descent from `a`, even though `a` is absent from commit3's
        // real parent (commit2).
        let backend = repo.store().backend_impl::<VersionStoreBackend>().unwrap();
        let phantom_copy_id = backend.write_copy_from(&c, a_copy_id).await.unwrap();
        let store = repo.store().clone();
        let commit2_tree_id = repo
            .store()
            .get_commit_async(&commit2)
            .await
            .unwrap()
            .tree()
            .tree_ids()
            .as_resolved()
            .cloned()
            .unwrap();
        let mut builder = TreeBuilder::new(store.clone(), commit2_tree_id);
        builder.set(
            c.clone(),
            TreeValue::File {
                id: jj_lib::backend::FileId::from_bytes(b"phantom-content-id-000000000000"),
                executable: false,
                copy_id: phantom_copy_id,
            },
        );
        let commit3_tree_id = builder.write_tree().await.unwrap();
        let merged_tree = MergedTree::resolved(store, commit3_tree_id);
        let mut tx = repo.start_transaction();
        tx.repo_mut()
            .new_commit(vec![commit2.clone()], merged_tree)
            .set_description("plant phantom copy record")
            .write()
            .await
            .unwrap();
        let repo = tx.commit("plant").await.unwrap();
        let commit3 = repo.view().heads().iter().next().unwrap().clone();

        let backend = repo.store().backend_impl::<VersionStoreBackend>().unwrap();
        let records = copy_records_between(backend, None, &commit2, &commit3)
            .await
            .unwrap();
        assert!(
            records.is_empty(),
            "a copy record with an absent source must be skipped, not fabricated: {records:?}"
        );
    }
}
