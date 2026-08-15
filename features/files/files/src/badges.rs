//! Badge derivation for root browsing (issue #266): the
//! resident-vs-stub and divergence state
//! [`files_proto::BrowseEntry`] carries.
//!
//! Both badges are *derived from the version store*, never stored twice
//! (ADR 0001's "derived, never a second authority" doctrine, the same
//! rule per-file chains follow):
//!
//! - **Stub** — a path the store tracks that is not resident in the
//!   live tree. Browsing lists it with its version-store identity so a
//!   240 GB project can be explored without hydrating it (glossary
//!   "Pointer stub"; on-demand hydration is issue #263).
//! - **Divergent** — the root's op log has more than one visible head
//!   and this path's state differs between them (glossary "Divergent
//!   versions": concurrent saves survive side by side). Resolution
//!   (pick A / pick B / keep both) is issue #267.
//!
//! **Every visible head contributes rows**, not just the one
//! `head_of` happens to pick: a file another replica saved and this
//! machine has never hydrated exists *only* on the other head, and a
//! divergence nobody can see is a divergence nobody can resolve (PR
//! #288 review). The listing is therefore the union of the live tree
//! and every head's tree; the chosen head only decides which identity
//! is reported when the heads agree.
//!
//! Reads go through jj's `Backend` trait, not `VersionStoreBackend`, so
//! a software root's colocated git store answers the same way a media
//! root's CAS does (issue #273 generalized the chain walk the same way).

use std::collections::{BTreeMap, BTreeSet};

use files_proto::BrowseEntry;
use jj_lib::backend::{Backend, CommitId, Tree, TreeValue};
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};

use crate::error::{Error, Result};

/// A tree entry's state, comparable across heads: the content address
/// for a file, the subtree id for a directory. Two heads agreeing on
/// this key agree on the entry.
type EntryState = (bool, String);

fn state_of(value: &TreeValue) -> Option<EntryState> {
    match value {
        TreeValue::File { id, .. } => Some((false, id.hex())),
        TreeValue::Tree(id) => Some((true, id.hex())),
        _ => None,
    }
}

/// The tree at `dir` inside `commit_id`, or `None` when that commit
/// doesn't have the directory (a path that exists on only one side of a
/// divergence, or below a file).
async fn dir_tree(
    backend: &dyn Backend,
    commit_id: &CommitId,
    dir: &RepoPath,
) -> Result<Option<Tree>> {
    let commit = backend.read_commit(commit_id).await?;
    let tree_id = commit
        .root_tree
        .clone()
        .into_resolved()
        .map_err(|_| Error::Repo("browsing a conflicted tree is unsupported (v1)".into()))?;
    let mut current = backend.read_tree(RepoPath::root(), &tree_id).await?;
    let mut walked = RepoPathBuf::root().to_owned();
    for component in dir.components() {
        walked = walked.join(component);
        match current.value(component) {
            Some(TreeValue::Tree(id)) => current = backend.read_tree(&walked, id).await?,
            _ => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// One head's listing of `dir`: entry name ⇒ its state.
pub(crate) async fn listing(
    backend: &dyn Backend,
    commit_id: &CommitId,
    dir: &RepoPath,
) -> Result<BTreeMap<String, EntryState>> {
    let mut out = BTreeMap::new();
    let Some(tree) = dir_tree(backend, commit_id, dir).await? else {
        return Ok(out);
    };
    for name in tree.names() {
        let Some(value) = tree.value(name) else {
            continue;
        };
        if let Some(state) = state_of(value) {
            out.insert(name.as_internal_str().to_owned(), state);
        }
    }
    Ok(out)
}

/// What the store knows about one directory, folded across every
/// visible head.
pub struct TrackedDir {
    /// Every name any head tracks ⇒ its state on the chosen head when
    /// that head has it, else its state on whichever other head does.
    entries: BTreeMap<String, EntryState>,
    /// Names whose state differs between heads (including present on
    /// one side, absent on another).
    divergent: BTreeSet<String>,
}

impl TrackedDir {
    /// Nothing tracked here — what a root whose store isn't open (see
    /// `backend`'s read paths) or a directory no head knows folds to.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
            divergent: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Fold every visible head's listing of `dir` into one [`TrackedDir`].
/// `head` is the checkpoint head the rest of the surface reads; it wins
/// when reporting an entry's kind, but it never decides *which* entries
/// exist.
///
/// Driven with `pollster::block_on` like every other jj-lib touch in
/// this crate (see `backend`'s module doc).
pub fn tracked_dir(
    backend: &dyn Backend,
    head: &CommitId,
    heads: &BTreeSet<CommitId>,
    dir: &RepoPath,
) -> Result<TrackedDir> {
    // The chosen head first, so its state is the one that survives the
    // `or_insert` below when heads agree.
    let mut ordered: Vec<&CommitId> = vec![head];
    ordered.extend(heads.iter().filter(|h| *h != head));

    let mut entries: BTreeMap<String, EntryState> = BTreeMap::new();
    let mut per_head: Vec<BTreeMap<String, EntryState>> = Vec::new();
    for commit_id in ordered {
        let one = pollster::block_on(listing(backend, commit_id, dir))?;
        for (name, state) in &one {
            entries.entry(name.clone()).or_insert_with(|| state.clone());
        }
        per_head.push(one);
    }

    let mut divergent = BTreeSet::new();
    if per_head.len() > 1 {
        for name in entries.keys() {
            // Distinct states across the heads — `None` (absent on that
            // side) counts as its own state, so an add on one side and
            // not the other is divergence just like differing content.
            let states: BTreeSet<Option<&EntryState>> =
                per_head.iter().map(|one| one.get(name)).collect();
            if states.len() > 1 {
                divergent.insert(name.clone());
            }
        }
    }

    Ok(TrackedDir { entries, divergent })
}

/// Annotate `entries` (the live-tree listing of `dir`) with stub +
/// divergence state, appending one row per tracked-but-not-resident
/// path — from any head, so a concurrent save this machine has never
/// hydrated is visible and resolvable.
pub fn annotate(tracked: &TrackedDir, entries: &mut Vec<BrowseEntry>) {
    for entry in entries.iter_mut() {
        entry.divergent = tracked.divergent.contains(&entry.name);
    }

    let resident: BTreeSet<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    let mut stubs: Vec<BrowseEntry> = tracked
        .entries
        .iter()
        .filter(|(name, _)| !resident.contains(name.as_str()))
        .map(|(name, (is_dir, _))| BrowseEntry {
            name: name.clone(),
            is_dir: *is_dir,
            size: None,
            stub: true,
            divergent: tracked.divergent.contains(name),
        })
        .collect();
    entries.append(&mut stubs);
    entries.sort_by(|a, b| a.name.cmp(&b.name));
}

/// A browse `subpath` as a jj repo path — the same normalization the
/// scan walker applies (native separators to `/`, no leading or
/// trailing slash). An empty subpath is the root itself.
pub fn repo_dir(subpath: &str) -> Result<RepoPathBuf> {
    let normalized = subpath
        .replace(std::path::MAIN_SEPARATOR, "/")
        .trim_matches('/')
        .to_owned();
    if normalized.is_empty() {
        return Ok(RepoPathBuf::root().to_owned());
    }
    // Belt and braces with jj's own validation: a `.`/`..` component
    // must never reach the tracked-path lookup, because that lookup is
    // the one browse path that doesn't go through a canonicalize +
    // prefix check (see `browse_inner`).
    if normalized.split('/').any(|c| c == "." || c == "..") {
        return Err(Error::BadRequest(format!(
            "subpath escapes the root: {subpath}"
        )));
    }
    RepoPathBuf::from_internal_string(&normalized)
        .map_err(|e| Error::BadRequest(format!("{normalized:?}: {e}")))
}
