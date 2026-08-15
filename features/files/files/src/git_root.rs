//! The colocated-git half of a **software** File Root (issue #273, ADR
//! 0001: "software roots use stock colocated git — a perfectly normal
//! `.git` for GitHub, CI, IDEs").
//!
//! Colocated means two views of one history. jj-lib's `GitBackend`
//! already makes every Files checkpoint a real git commit in the root's
//! own object database; what this module adds is the rest of what "a
//! normal git repo" means to git tooling:
//!
//! - **Refs.** A commit no ref points at is unreachable — `git log` would
//!   show nothing and `git push` would have nothing to send. After every
//!   checkpoint the root's checked-out branch is moved to the new commit
//!   ([`publish_checkpoint`]) via jj's own `export_refs`, so clone, fetch,
//!   push, and CI see ordinary branch history.
//! - **The index.** Git compares worktree ⇄ index ⇄ HEAD; a repo whose
//!   index is missing reports every tracked file as deleted-and-untracked,
//!   which no IDE would call normal. [`publish_checkpoint`] rewrites the
//!   index from the checkpoint's tree, so `git status` is clean right
//!   after a checkpoint — the same thing jj does when it moves the
//!   working-copy commit.
//! - **The other direction.** Commits a human (or CI) makes with plain
//!   `git` are imported into the jj view on every open
//!   ([`import_from_git`]), so the Files chain/history RPC reflects them
//!   and the next checkpoint builds on top of them instead of forking
//!   history behind git's back.
//!
//! Adoption falls out of the same seam: pointing `create_root` at a
//! folder that already contains `.git` keeps that repository (and its
//! remotes, and its history) and layers Files on top.

use std::collections::HashMap;
use std::sync::Arc;

use jj_lib::backend::CommitId;
use jj_lib::git::{GitImportOptions, GitSettings};
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_store::RefTarget;
use jj_lib::ref_name::{RefName, RefNameBuf};
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::settings::UserSettings;

use crate::error::{Error, Result};

/// Where a software root's `HEAD` points, which decides both what a
/// checkpoint parents onto and what it moves afterwards.
///
/// Distinguishing these matters (PR #282 review): gix returns
/// `head_name() == Ok(None)` *only* for a detached HEAD — an unborn HEAD
/// still names its branch — so a "fall back to `main`" default would fire
/// exactly where it does damage, moving an unrelated branch and yanking a
/// user off a deliberate detached checkout (a tag, a CI checkout).
#[derive(Debug, Clone)]
pub enum HeadRef {
    /// HEAD points at a branch (born or unborn). Checkpoints move that
    /// branch and HEAD stays attached to it.
    Branch(RefNameBuf),
    /// HEAD points straight at a commit. Checkpoints commit on top of it
    /// and move HEAD itself — precisely what `git commit` does on a
    /// detached HEAD — touching no branch at all.
    Detached(CommitId),
}

fn import_options(settings: &UserSettings) -> Result<GitImportOptions> {
    let git_settings = GitSettings::from_settings(settings)
        .map_err(|e| Error::Repo(format!("reading git settings: {e}")))?;
    Ok(GitImportOptions {
        abandon_unreachable_commits: git_settings.abandon_unreachable_commits,
        record_synthetic_predecessors: git_settings.record_synthetic_predecessors,
        remote_auto_track_bookmarks: HashMap::new(),
    })
}

/// Read git's own `HEAD` — the branch it is on (`main`, `master`,
/// whatever the adopted repo used), or the commit it is detached at.
pub fn head_ref(repo: &Arc<ReadonlyRepo>) -> Result<HeadRef> {
    let git_repo = jj_lib::git::get_git_repo(repo.store())
        .map_err(|e| Error::Repo(format!("not a git-backed root: {e}")))?;
    let head = git_repo
        .head()
        .map_err(|e| Error::Repo(format!("reading git HEAD: {e}")))?;
    match &head.kind {
        gix::head::Kind::Symbolic(reference) => Ok(HeadRef::Branch(branch_name(
            reference.name.shorten().to_string(),
        ))),
        gix::head::Kind::Unborn(name) => {
            Ok(HeadRef::Branch(branch_name(name.shorten().to_string())))
        }
        gix::head::Kind::Detached { target, peeled } => Ok(HeadRef::Detached(
            CommitId::from_bytes(peeled.unwrap_or(*target).as_bytes()),
        )),
    }
}

fn branch_name(shortened: String) -> RefNameBuf {
    RefNameBuf::from(shortened)
}

/// The commit a software root's next checkpoint builds on: the tip of the
/// checked-out branch (or the root commit when that branch is unborn), or
/// the commit HEAD is detached at.
///
/// Deliberately *not* `view().heads().next()` (what media roots use):
/// an adopted repo can have many branches, and picking an arbitrary head
/// would silently commit onto whichever one sorted first.
pub fn head_commit(repo: &Arc<ReadonlyRepo>) -> Result<CommitId> {
    match head_ref(repo)? {
        HeadRef::Detached(id) => Ok(id),
        HeadRef::Branch(bookmark) => {
            let target = repo.view().get_local_bookmark(&bookmark);
            Ok(target
                .as_normal()
                .cloned()
                .unwrap_or_else(|| repo.store().root_commit_id().clone()))
        }
    }
}

/// Marks our own block in `info/exclude`, so re-running this recognizes
/// *its own* previous write rather than any line that merely mentions the
/// store directory (a user's own `.fts-files` rule, say).
const EXCLUDE_SENTINEL: &str = "# fts-files:root-internals:v1";

/// What Files itself keeps in the tree, hidden from git.
const EXCLUDE_BLOCK: &str = "\
# fts-files:root-internals:v1
# Files (Task) root internals — the version store and the root marker.
# Written to .git/info/exclude rather than .gitignore: this is Files'
# business with this checkout, not a project decision to commit.
/.fts-files/
/.fts-root.json
";

/// Teach git to ignore a File Root's own internals, via the repo-local
/// `info/exclude` (never the project's `.gitignore`, which belongs to the
/// project and would end up in its commits). Without this, `git status`
/// on a perfectly clean software root reports two untracked entries that
/// no developer put there. Idempotent.
pub fn exclude_root_internals(repo: &Arc<ReadonlyRepo>) -> Result<()> {
    let git_repo = jj_lib::git::get_git_repo(repo.store())
        .map_err(|e| Error::Repo(format!("not a git-backed root: {e}")))?;
    let path = git_repo.common_dir().join("info").join("exclude");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains(EXCLUDE_SENTINEL) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut contents = existing;
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(EXCLUDE_BLOCK);
    std::fs::write(&path, contents)?;
    Ok(())
}

/// Import whatever git has done since we last looked — an adopted
/// repository's existing history on first touch, plus any commits,
/// fetches, or branch moves made with plain `git` between RPC calls.
pub fn import_from_git(repo: Arc<ReadonlyRepo>) -> Result<Arc<ReadonlyRepo>> {
    let options = import_options(repo.settings())?;
    let mut tx = repo.start_transaction();
    pollster::block_on(jj_lib::git::import_refs(tx.repo_mut(), &options))
        .map_err(|e| Error::Repo(format!("importing git refs: {e}")))?;
    pollster::block_on(jj_lib::git::import_head(tx.repo_mut()))
        .map_err(|e| Error::Repo(format!("importing git HEAD: {e}")))?;
    if tx.repo().has_changes() {
        pollster::block_on(tx.commit("import git refs")).map_err(|e| Error::Repo(e.to_string()))
    } else {
        Ok(repo)
    }
}

/// Make `commit_id` what git sees: the tip of the root's checked-out
/// branch (or `HEAD` itself when detached), with the index rewritten to
/// match. After this, `git log`, `git status`, `git clone`, and `git push`
/// behave exactly as they would in a repository a human had committed to.
///
/// Fails rather than half-publishing: if git refuses the ref move —
/// because someone moved that branch with `git` since our last import —
/// the branch, HEAD, and index are all left alone and the divergence is
/// reported (PR #282 review).
pub fn publish_checkpoint(
    repo: Arc<ReadonlyRepo>,
    commit_id: &CommitId,
) -> Result<Arc<ReadonlyRepo>> {
    let head = head_ref(&repo)?;
    let repo = match &head {
        HeadRef::Branch(bookmark) => export_bookmark(repo, bookmark, commit_id)?,
        HeadRef::Detached(_) => record_git_head(repo, commit_id)?,
    };
    match &head {
        HeadRef::Branch(bookmark) => {
            verify_branch(&repo, bookmark, commit_id)?;
            attach_head(&repo, bookmark)?;
        }
        // Detached: move HEAD itself, touching no branch — the same thing
        // `git commit` does on a detached checkout.
        HeadRef::Detached(_) => set_detached_head(&repo, commit_id)?,
    }
    write_git_index(&repo, commit_id)?;
    Ok(repo)
}

/// Move the local bookmark and export it to `refs/heads/<bookmark>`.
///
/// `export_refs` reports refusals through its return value, *not* through
/// `Err`: a branch that moved on the git side since jj last imported it
/// lands in `GitExportStats.failed_bookmarks` while the call still
/// succeeds (jj-lib 0.44 `git.rs`, whose doc says conflicted refs are
/// left for the next import). Dropping that would leave the git branch at
/// the human's commit while we rewrote HEAD and the index to ours.
fn export_bookmark(
    repo: Arc<ReadonlyRepo>,
    bookmark: &RefNameBuf,
    commit_id: &CommitId,
) -> Result<Arc<ReadonlyRepo>> {
    let mut tx = repo.start_transaction();
    tx.repo_mut().set_local_bookmark_target(
        RefName::new(bookmark.as_str()),
        RefTarget::normal(commit_id.clone()),
    );
    // jj records what it believes git's HEAD is; keeping it in step means
    // the next `import_from_git` sees "nothing moved" rather than
    // mistaking our own export for a git-side change.
    tx.repo_mut()
        .set_git_head_target(RefTarget::normal(commit_id.clone()));
    let stats = jj_lib::git::export_refs(tx.repo_mut())
        .map_err(|e| Error::Repo(format!("exporting git refs: {e}")))?;
    if !stats.failed_bookmarks.is_empty() || !stats.failed_tags.is_empty() {
        return Err(Error::Repo(format!(
            "git refused to update {} ref(s) — the repository moved outside Files since its \
             history was last read; the checkpoint was not published: {:?}",
            stats.failed_bookmarks.len() + stats.failed_tags.len(),
            stats.failed_bookmarks,
        )));
    }
    pollster::block_on(tx.commit("export git refs")).map_err(|e| Error::Repo(e.to_string()))
}

/// The detached-HEAD counterpart of [`export_bookmark`]: no bookmark, no
/// export — only jj's record of where git's HEAD now is.
fn record_git_head(repo: Arc<ReadonlyRepo>, commit_id: &CommitId) -> Result<Arc<ReadonlyRepo>> {
    let mut tx = repo.start_transaction();
    tx.repo_mut()
        .set_git_head_target(RefTarget::normal(commit_id.clone()));
    pollster::block_on(tx.commit("record git HEAD")).map_err(|e| Error::Repo(e.to_string()))
}

/// Confirm `refs/heads/<bookmark>` really is at `commit_id` before HEAD
/// and the index are pointed at it — the belt to `export_bookmark`'s
/// braces, catching any refusal jj reports some other way.
fn verify_branch(
    repo: &Arc<ReadonlyRepo>,
    bookmark: &RefNameBuf,
    commit_id: &CommitId,
) -> Result<()> {
    let git_repo = jj_lib::git::get_git_repo(repo.store())
        .map_err(|e| Error::Repo(format!("not a git-backed root: {e}")))?;
    let full = format!("refs/heads/{}", bookmark.as_str());
    let actual = git_repo
        .find_reference(full.as_str())
        .map_err(|e| Error::Repo(format!("reading {full}: {e}")))?
        .target()
        .id()
        .to_owned();
    if actual.as_bytes() != commit_id.as_bytes() {
        return Err(Error::Repo(format!(
            "{full} is at {actual}, not the checkpoint {}: the repository moved outside Files \
             since its history was last read",
            commit_id.hex(),
        )));
    }
    Ok(())
}

/// Point git's `HEAD` straight at `commit_id`, leaving it detached.
fn set_detached_head(repo: &Arc<ReadonlyRepo>, commit_id: &CommitId) -> Result<()> {
    let git_repo = jj_lib::git::get_git_repo(repo.store())
        .map_err(|e| Error::Repo(format!("not a git-backed root: {e}")))?;
    edit_head(
        &git_repo,
        gix::refs::Target::Object(gix::ObjectId::from_bytes_or_panic(commit_id.as_bytes())),
    )
}

/// Point git's `HEAD` back at `refs/heads/<bookmark>`.
///
/// jj's `export_refs` deliberately *detaches* `HEAD` whenever it moves the
/// branch `HEAD` is on — correct for jj's own working-copy model, where
/// the checkout is jj's to own. A software File Root is the opposite
/// case: the checkout belongs to whoever opens the folder, and a
/// permanently detached `HEAD` is exactly the "not a normal repository"
/// state this flavor exists to avoid (`git status` says "HEAD detached",
/// `git push` needs an explicit refspec, IDEs show no branch). So the
/// branch is re-attached after every export — it already points at the
/// checkpoint we just wrote (checked by `verify_branch`), so attaching
/// changes nothing about what is reachable, only how git presents it.
///
/// A root whose HEAD was *already* detached never reaches here: that is
/// its own [`HeadRef`] case, and it keeps its detached checkout.
fn attach_head(repo: &Arc<ReadonlyRepo>, bookmark: &RefNameBuf) -> Result<()> {
    let git_repo = jj_lib::git::get_git_repo(repo.store())
        .map_err(|e| Error::Repo(format!("not a git-backed root: {e}")))?;
    let branch_ref: gix::refs::FullName = format!("refs/heads/{}", bookmark.as_str())
        .try_into()
        .map_err(|e| Error::Repo(format!("invalid branch name {bookmark:?}: {e}")))?;
    edit_head(&git_repo, gix::refs::Target::Symbolic(branch_ref))
}

fn edit_head(git_repo: &gix::Repository, new: gix::refs::Target) -> Result<()> {
    git_repo
        .edit_reference(gix::refs::transaction::RefEdit {
            change: gix::refs::transaction::Change::Update {
                log: gix::refs::transaction::LogChange {
                    message: "checkpoint (Files)".into(),
                    ..Default::default()
                },
                expected: gix::refs::transaction::PreviousValue::Any,
                new,
            },
            name: "HEAD"
                .try_into()
                .expect("HEAD is a valid full reference name"),
            deref: false,
        })
        .map_err(|e| Error::Repo(format!("updating git HEAD: {e}")))?;
    Ok(())
}

/// Rewrite `.git/index` from `commit_id`'s tree so git sees a clean
/// worktree. Mirrors jj-lib's own `reset_index` (private to that crate)
/// for the resolved-tree case; a conflicted tree can't occur here
/// because a checkpoint always writes a resolved tree.
fn write_git_index(repo: &Arc<ReadonlyRepo>, commit_id: &CommitId) -> Result<()> {
    let git_repo = jj_lib::git::get_git_repo(repo.store())
        .map_err(|e| Error::Repo(format!("not a git-backed root: {e}")))?;
    let commit = pollster::block_on(repo.store().get_commit_async(commit_id))?;
    let tree_id =
        commit.tree_ids().as_resolved().cloned().ok_or_else(|| {
            Error::Repo("checkpoint wrote a conflicted tree (unsupported)".into())
        })?;

    let mut index = if &tree_id == repo.store().empty_tree_id() {
        // Git doesn't require the empty tree to be present in the object
        // database, so gix can fail to load it — use an empty index.
        gix::index::File::from_state(
            gix::index::State::new(git_repo.object_hash()),
            git_repo.index_path(),
        )
    } else {
        git_repo
            .index_from_tree(&gix::ObjectId::from_bytes_or_panic(tree_id.as_bytes()))
            .map_err(|e| Error::Repo(format!("building the git index: {e}")))?
    };
    index
        .write(gix::index::write::Options::default())
        .map_err(|e| Error::Repo(format!("writing the git index: {e}")))?;
    Ok(())
}
