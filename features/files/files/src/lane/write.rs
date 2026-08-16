//! `WriteService` — structural mutation.
//!
//! The dominant gap in v1: there were no write RPCs at all, so nothing
//! could be created, renamed, moved or deleted over the network. Most of
//! what a cloud provider calls file management is this lane, and every
//! other lane's story about history depends on it behaving.
//!
//! ## The shape every method shares
//!
//! One shape, four phases, because the spec's four properties
//! (transactional / one operation / set-at-a-time / never choose for the
//! user) are all properties of the *batch* rather than of a single path:
//!
//! 1. **Plan.** Validate every path, resolve every disk location, settle
//!    every collision against the caller's [`OnConflict`], and reserve
//!    every destination. Nothing on disk is touched. A batch that cannot
//!    be planned in full returns here, with the tree untouched — which is
//!    the cheap half of "wholly or not at all".
//! 2. **Apply.** Run the planned acts in order, recording an undo for
//!    each as it succeeds. The first failure rolls the journal back in
//!    reverse and the batch reports the fault.
//! 3. **Certify.** One checkpoint over the whole batch, so history shows
//!    "moved 30 takes" rather than thirty renames. This is
//!    `files.write.surface`'s "wrapped in one jj operation" clause, and it
//!    is why the checkpoint is taken once at the end rather than per path.
//! 4. **Commit.** Purge the staging area. Until this point every
//!    displaced or deleted path is still sitting in staging under a
//!    rename, which is what makes step 2's rollback able to undo a
//!    *destructive* act rather than only an additive one.
//!
//! ## Deletion is a checkpoint, not a trash move
//!
//! `delete_paths` renames its victims into the root's store directory
//! (invisible to the certifying scan, which skips `STORE_DIR`), certifies
//! the tree without them, and only then unlinks. Recovery is
//! [`VersionService`](files_proto::service::version::VersionService)
//! reading the parent checkpoint — there is no second collector to
//! maintain, nothing is stored twice, and "recently deleted" is a lens
//! over history rather than a place.
//!
//! The same mechanism serves [`OnConflict::Replace`]: the displaced file
//! is not discarded, it is left behind in the checkpoint before this one,
//! which is exactly "records a new version rather than discarding the
//! old".
//!
//! ## Why the root lock is dropped before the checkpoint
//!
//! `capture_inner` takes `root_lock` itself and the lock is a plain
//! non-reentrant `Mutex`, so holding it across the certify phase would
//! deadlock the batch against its own checkpoint. The filesystem work
//! runs under the lock and the lock is released before certifying; a
//! writer that interleaves there is certified *with* us rather than
//! against us, because the capture is a full certifying scan of the live
//! tree rather than a diff of what we claimed to change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use files_proto::error::FilesFault;
use files_proto::id::RootId;
use files_proto::model::FileRootInfo;
use files_proto::path::RootPath;
use files_proto::service::media::ByteTicket;
use files_proto::service::write::{OnConflict, Outcome, Relocation, WriteReceipt, WriteService};

use crate::backend::FilesBackend;

/// How many derived names `KeepBoth` will try before giving up.
///
/// A bound rather than an unbounded loop: a directory holding a thousand
/// `mix (n).wav` is a caller bug or an attack, and spinning on `stat` is
/// the wrong way to find that out.
const KEEP_BOTH_TRIES: u32 = 1_000;

/// The single into-self check.
///
/// `files.write.surface` wants this once rather than at each call site —
/// move and copy both route through here, and the equality case counts:
/// a destination that *is* its source is the degenerate member of the
/// same family, and answering `IntoSelf` says why rather than leaving the
/// caller with a mysterious `Exists`.
// t[impl files.write.surface] — the one into-self guard
fn guard_into_self(from: &RootPath, to: &RootPath) -> Result<(), FilesFault> {
    if to.is_within(from) {
        return Err(FilesFault::IntoSelf {
            from: from.clone(),
            to: to.clone(),
        });
    }
    Ok(())
}

/// Re-validate a wire path and refuse the root itself.
///
/// `RootPath` is `#[serde(transparent)]`, so a hostile peer's `..`
/// arrives having never seen `parse` — the guard has to run on this side.
/// The root is refused separately because it is a legal `RootPath` and an
/// illegal *subject*: renaming, moving or deleting a root is
/// `RootsService`'s business, and letting it through here would make
/// `guard_into_self` vacuously true for every destination.
fn addressable(path: &RootPath) -> Result<RootPath, FilesFault> {
    let path = path.validate()?;
    if path.is_root() {
        return Err(FilesFault::invalid(
            "the root itself is not a path this lane can create, move or delete",
        ));
    }
    Ok(path)
}

/// One planned filesystem act, with enough in it to be undone.
#[derive(Debug)]
enum Act {
    /// Create this directory and any missing ancestors.
    MakeDir(PathBuf),
    /// Rename the existing occupant of this location into staging,
    /// where it stays until the batch commits.
    Displace(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
}

/// What was actually done, in the order it happened.
#[derive(Debug)]
enum Undo {
    Created(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Copied(PathBuf),
    Displaced { original: PathBuf, staged: PathBuf },
}

/// One transactional batch against one root.
struct Batch<'a> {
    backend: &'a FilesBackend,
    root: &'a FileRootInfo,
    /// Where displaced and deleted paths wait out the batch. Inside the
    /// root so every rename is same-filesystem (a cross-device rename is
    /// a copy, which would make "stage it aside" cost the size of the
    /// file), and inside `STORE_DIR` so the certifying scan skips it.
    staging: PathBuf,
    /// Destinations this batch has already claimed, so two paths in one
    /// set cannot silently land on each other.
    reserved: HashSet<RootPath>,
    undo: Vec<Undo>,
    staged: u64,
}

impl<'a> Batch<'a> {
    fn new(backend: &'a FilesBackend, root: &'a FileRootInfo) -> Self {
        let staging = crate::repo_open::store_dir(Path::new(&root.path))
            .join("write-staging")
            .join(uuid::Uuid::new_v4().to_string());
        Self {
            backend,
            root,
            staging,
            reserved: HashSet::new(),
            undo: Vec::new(),
            staged: 0,
        }
    }

    /// The confined on-disk location of a root-relative path.
    fn disk(&self, path: &RootPath) -> Result<PathBuf, FilesFault> {
        self.backend
            .resolve_root_file(self.root, path.as_str())
            .map(|(disk, _)| disk)
            .map_err(FilesFault::from)
    }

    /// Reserve a destination, refusing a second claim on it.
    ///
    /// Within-batch collisions are not conflicts in the `OnConflict`
    /// sense — there is no existing file to keep, replace or keep both of
    /// — they are a contradictory request, and the honest answer is that
    /// the destination is taken.
    fn reserve(&mut self, path: RootPath) -> Result<RootPath, FilesFault> {
        if !self.reserved.insert(path.clone()) {
            return Err(FilesFault::Exists { path });
        }
        Ok(path)
    }

    /// Settle a collision, never choosing for the caller.
    ///
    /// Returns the destination the incoming path should take and whether
    /// the occupant has to be displaced first, or `None` when the caller
    /// asked to keep what is already there.
    // t[impl files.write.surface] — Fail / KeepBoth / Replace / KeepExisting
    fn settle(
        &mut self,
        dest: RootPath,
        on_conflict: OnConflict,
    ) -> Result<Option<(RootPath, bool)>, FilesFault> {
        let occupied = self.disk(&dest)?.symlink_metadata().is_ok();
        if !occupied {
            return Ok(Some((self.reserve(dest)?, false)));
        }
        match on_conflict {
            OnConflict::Fail => Err(FilesFault::Exists { path: dest }),
            OnConflict::KeepExisting => Ok(None),
            OnConflict::Replace => Ok(Some((self.reserve(dest)?, true))),
            OnConflict::KeepBoth => {
                let free = self.keep_both(&dest)?;
                Ok(Some((self.reserve(free)?, false)))
            }
        }
    }

    /// A free name beside `taken`, in the same parent.
    ///
    /// The suffix goes before the extension because the extension is what
    /// decides which application opens the file: `mix (2).wav` stays a
    /// wave file, `mix.wav (2)` does not.
    fn keep_both(&self, taken: &RootPath) -> Result<RootPath, FilesFault> {
        let parent = taken.parent().unwrap_or_else(RootPath::root);
        let name = taken.name().unwrap_or_default();
        let (stem, ext) = match name.rsplit_once('.') {
            // A leading dot is not an extension — `.gitignore` has no
            // stem to suffix, so the whole name is the stem.
            Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
            _ => (name, String::new()),
        };
        for n in 2..=KEEP_BOTH_TRIES {
            let candidate = parent.join(format!("{stem} ({n}){ext}"))?;
            if !self.reserved.contains(&candidate)
                && self.disk(&candidate)?.symlink_metadata().is_err()
            {
                return Ok(candidate);
            }
        }
        Err(FilesFault::Exists {
            path: taken.clone(),
        })
    }

    /// The destination's parent must already exist.
    ///
    /// Deliberately not created on the fly: a typo in a move destination
    /// would otherwise silently manufacture a folder tree, and
    /// `create_dirs` is right there for a caller that meant it.
    fn require_parent(&self, dest: &RootPath) -> Result<(), FilesFault> {
        let Some(parent) = dest.parent() else {
            return Ok(()); // a top-level destination's parent is the root
        };
        if self.disk(&parent)?.is_dir() {
            return Ok(());
        }
        Err(FilesFault::PathNotFound(parent))
    }

    /// The source must exist, or there is nothing to move, copy or delete.
    fn require_source(&self, from: &RootPath) -> Result<PathBuf, FilesFault> {
        let disk = self.disk(from)?;
        if disk.symlink_metadata().is_err() {
            return Err(FilesFault::PathNotFound(from.clone()));
        }
        Ok(disk)
    }

    /// Run the plan, rolling back the whole batch on the first failure.
    // t[impl files.write.surface] — wholly, or the tree is untouched
    fn apply(&mut self, acts: Vec<Act>) -> Result<(), FilesFault> {
        for act in acts {
            if let Err(err) = self.step(act) {
                self.rollback();
                return Err(err);
            }
        }
        Ok(())
    }

    fn step(&mut self, act: Act) -> Result<(), FilesFault> {
        match act {
            Act::MakeDir(disk) => self.make_dir(&disk),
            Act::Displace(disk) => self.displace(&disk),
            Act::Rename { from, to } => {
                std::fs::rename(&from, &to).map_err(FilesFault::io)?;
                self.undo.push(Undo::Renamed { from, to });
                Ok(())
            }
            Act::Copy { from, to } => {
                // Recorded before the copy rather than after: a copy that
                // fails half way has still written, and the undo must
                // reach that debris.
                self.undo.push(Undo::Copied(to.clone()));
                copy_tree(&from, &to)
            }
        }
    }

    /// Create a directory and its missing ancestors, recording each one.
    ///
    /// `create_dir_all` would be one call, and would leave us unable to
    /// say which directories it made — so a rollback would either strand
    /// them or remove a directory that was already there. Walking up to
    /// the first existing ancestor and creating downwards is what makes
    /// the undo exact.
    // t[impl files.write.surface] — an existing directory is not an error
    fn make_dir(&mut self, disk: &Path) -> Result<(), FilesFault> {
        if disk.is_dir() {
            return Ok(()); // idempotent: a retried request must not fail
        }
        let mut missing = Vec::new();
        let mut at = disk;
        while !at.exists() {
            missing.push(at.to_path_buf());
            match at.parent() {
                Some(parent) => at = parent,
                None => break,
            }
        }
        for dir in missing.into_iter().rev() {
            std::fs::create_dir(&dir).map_err(FilesFault::io)?;
            self.undo.push(Undo::Created(dir));
        }
        Ok(())
    }

    /// Move an existing path out of the tree and into staging.
    ///
    /// The staged name is a counter rather than the original name: two
    /// paths in one batch can share a basename, and staging is flat.
    fn displace(&mut self, disk: &Path) -> Result<(), FilesFault> {
        std::fs::create_dir_all(&self.staging).map_err(FilesFault::io)?;
        self.staged += 1;
        let staged = self.staging.join(self.staged.to_string());
        std::fs::rename(disk, &staged).map_err(FilesFault::io)?;
        self.undo.push(Undo::Displaced {
            original: disk.to_path_buf(),
            staged,
        });
        Ok(())
    }

    /// Put the tree back, newest act first.
    ///
    /// Best-effort by necessity: this runs *because* the filesystem
    /// already refused something, so an undo can fail too. Each failure
    /// is logged individually rather than aborting the unwind — the
    /// remaining acts are independent, and giving up on the first one
    /// would leave more of the batch applied than necessary.
    fn rollback(&mut self) {
        for act in self.undo.drain(..).rev() {
            let failed = match act {
                Undo::Created(dir) => std::fs::remove_dir(&dir).err().map(|e| (dir, e)),
                Undo::Renamed { from, to } => std::fs::rename(&to, &from).err().map(|e| (to, e)),
                Undo::Copied(path) => remove_any(&path).err().map(|e| (path, e)),
                Undo::Displaced { original, staged } => std::fs::rename(&staged, &original)
                    .err()
                    .map(|e| (original, e)),
            };
            if let Some((path, err)) = failed {
                tracing::warn!(path = %path.display(), %err, "files: write rollback step failed");
            }
        }
    }

    /// Drop the staging area once the batch is certified.
    ///
    /// After the checkpoint, because until then staging is the only place
    /// the displaced bytes exist. Failure here is a leak of a hidden
    /// directory, not a correctness problem, so it is logged rather than
    /// returned: the caller's operation genuinely succeeded.
    fn commit(&self) {
        if self.staged == 0 {
            return;
        }
        if let Err(err) = std::fs::remove_dir_all(&self.staging) {
            tracing::warn!(
                path = %self.staging.display(),
                %err,
                "files: write staging left behind"
            );
            return;
        }
        // And the shared parent, when this batch was the last one using
        // it. `remove_dir` refuses a non-empty directory, which is
        // exactly the test for "another batch is still in there" — so
        // the error is the answer rather than a failure.
        if let Some(parent) = self.staging.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// Remove a path whatever it is.
fn remove_any(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Copy a file or a whole subtree.
///
/// Byte-for-byte. `files.write.surface` wants an intra-root copy to cost
/// no bytes, which needs the copy to be recorded as a content-addressed
/// reference rather than performed on the filesystem — and the live tree
/// is the source of truth for every reader here, so a reference with no
/// file behind it would be invisible to `browse`, to the watcher and to
/// the applications that own the tree. The cheap copy arrives with
/// materialisation-on-read, not before it.
fn copy_tree(from: &Path, to: &Path) -> Result<(), FilesFault> {
    let meta = from.symlink_metadata().map_err(FilesFault::io)?;
    if !meta.is_dir() {
        std::fs::copy(from, to).map_err(FilesFault::io)?;
        return Ok(());
    }
    std::fs::create_dir(to).map_err(FilesFault::io)?;
    for entry in std::fs::read_dir(from).map_err(FilesFault::io)? {
        let entry = entry.map_err(FilesFault::io)?;
        copy_tree(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

/// Plan, apply, certify, commit — the whole of a batch, in one place.
///
/// `plan` runs with the root lock held and returns the acts plus the
/// outcomes they will produce, so every method here is a planner and
/// nothing else.
fn transact<P>(
    backend: &FilesBackend,
    root_id: RootId,
    description: String,
    plan: P,
) -> Result<WriteReceipt, FilesFault>
where
    P: FnOnce(&mut Batch<'_>) -> Result<(Vec<Act>, Vec<Outcome>), FilesFault>,
{
    let root = crate::lane::root_or_fault(backend, root_id)?;
    let mut batch = Batch::new(backend, &root);

    let outcomes = {
        // Writes on one root serialise: two batches interleaving their
        // renames would each roll back over the other's work. Released
        // at the end of this block — `capture_inner` takes the same
        // lock, and it is not reentrant (see the module doc).
        let lock = backend.root_lock(root_id.get());
        let _guard = lock.lock().expect("root write lock poisoned");
        let (acts, outcomes) = plan(&mut batch)?;
        batch.apply(acts)?;
        outcomes
    };

    // t[impl files.write.surface] — one operation per user action
    match backend.checkpoint_now_inner(root_id.get(), Some(description)) {
        Ok(info) => {
            batch.commit();
            // The catalogue is a separate structure and nothing else
            // updates it, so a write it never hears about is a write the
            // tree lane reports as not having happened. Folding it in
            // here — rather than dropping the catalogue — keeps the
            // change log, so subscribers get a delta instead of having
            // to re-list a tree that changed by one file.
            let touched: Vec<_> = outcomes
                .iter()
                .filter_map(|o| o.landed_at.clone())
                .collect();
            let removed: Vec<_> = outcomes
                .iter()
                .filter(|o| o.landed_at.is_none() && !o.skipped)
                .map(|o| o.path.clone())
                .collect();
            if let Ok(root) = crate::lane::root_or_fault(backend, root_id) {
                crate::lane::tree::note_write(backend, &root, &touched, &removed);
            }
            Ok(WriteReceipt {
                root_id,
                outcomes,
                operation: info.commit_id,
            })
        }
        Err(err) => {
            // History is the whole point of the write: a batch that
            // cannot be recorded is a batch that did not happen.
            batch.rollback();
            batch.commit();
            Err(FilesFault::from(err))
        }
    }
}

impl WriteService for FilesBackend {
    /// Create directories, missing parents included.
    ///
    /// Idempotent by design: a client retrying after a dropped connection
    /// has no way to know whether the first attempt landed, so an
    /// existing directory reports `skipped` rather than failing.
    // t[impl files.write.surface]
    async fn create_dirs(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
    ) -> Result<WriteReceipt, FilesFault> {
        let this = self.clone();
        let description = format!("create {} director{}", paths.len(), plural_y(paths.len()));
        crate::lane::blocking(move || {
            Ok(transact(&this, root_id, description, move |batch| {
                let mut acts = Vec::new();
                let mut outcomes = Vec::new();
                for path in &paths {
                    let path = addressable(path)?;
                    let disk = batch.disk(&path)?;
                    if disk.is_dir() {
                        outcomes.push(Outcome {
                            landed_at: Some(path.clone()),
                            path,
                            skipped: true,
                        });
                        continue;
                    }
                    if disk.symlink_metadata().is_ok() {
                        // Something is there and it is not a directory.
                        // Idempotence is about directories; silently
                        // treating a file as one is how a caller loses
                        // data it never mentioned.
                        return Err(FilesFault::NotADirectory(path));
                    }
                    acts.push(Act::MakeDir(disk));
                    outcomes.push(Outcome {
                        landed_at: Some(path.clone()),
                        path,
                        skipped: false,
                    });
                }
                Ok((acts, outcomes))
            }))
        })
        .await?
    }

    /// Rename in place — the parent is unchanged.
    // t[impl files.write.surface]
    async fn rename(
        &self,
        root_id: RootId,
        path: RootPath,
        name: String,
    ) -> Result<WriteReceipt, FilesFault> {
        let this = self.clone();
        let description = format!("rename {path} to {name}");
        crate::lane::blocking(move || {
            Ok(transact(&this, root_id, description, move |batch| {
                let from = addressable(&path)?;
                // `join` parses the component, so a name carrying a
                // separator or `..` is refused here rather than becoming
                // a move nobody asked for.
                let to = from.parent().unwrap_or_else(RootPath::root).join(&name)?;
                if to == from {
                    return Err(FilesFault::invalid("that is already its name"));
                }
                let from_disk = batch.require_source(&from)?;
                // Renaming onto an existing sibling is a collision the
                // caller has expressed no policy about — this signature
                // takes no `OnConflict`, so the only honest answer is to
                // refuse and let them choose.
                let to_disk = batch.disk(&to)?;
                if to_disk.symlink_metadata().is_ok() {
                    return Err(FilesFault::Exists { path: to });
                }
                Ok((
                    vec![Act::Rename {
                        from: from_disk,
                        to: to_disk,
                    }],
                    vec![Outcome {
                        path: from,
                        landed_at: Some(to),
                        skipped: false,
                    }],
                ))
            }))
        })
        .await?
    }

    /// Move a set of paths, atomically across the set.
    // t[impl files.write.surface]
    async fn move_paths(
        &self,
        root_id: RootId,
        moves: Vec<Relocation>,
        on_conflict: OnConflict,
    ) -> Result<WriteReceipt, FilesFault> {
        let this = self.clone();
        let description = format!("move {} path{}", moves.len(), plural_s(moves.len()));
        crate::lane::blocking(move || {
            Ok(transact(&this, root_id, description, move |batch| {
                let mut acts = Vec::new();
                let mut outcomes = Vec::new();
                for relocation in &moves {
                    let from = addressable(&relocation.from)?;
                    let to = addressable(&relocation.to)?;
                    guard_into_self(&from, &to)?;
                    let from_disk = batch.require_source(&from)?;
                    batch.require_parent(&to)?;

                    let Some((landed, displace)) = batch.settle(to, on_conflict)? else {
                        outcomes.push(Outcome {
                            path: from,
                            landed_at: None,
                            skipped: true,
                        });
                        continue;
                    };
                    let to_disk = batch.disk(&landed)?;
                    if displace {
                        acts.push(Act::Displace(to_disk.clone()));
                    }
                    acts.push(Act::Rename {
                        from: from_disk,
                        to: to_disk,
                    });
                    outcomes.push(Outcome {
                        path: from,
                        landed_at: Some(landed),
                        skipped: false,
                    });
                }
                Ok((acts, outcomes))
            }))
        })
        .await?
    }

    /// Copy a set of paths, atomically across the set.
    // t[impl files.write.surface]
    async fn copy_paths(
        &self,
        root_id: RootId,
        copies: Vec<Relocation>,
        on_conflict: OnConflict,
    ) -> Result<WriteReceipt, FilesFault> {
        let this = self.clone();
        let description = format!("copy {} path{}", copies.len(), plural_s(copies.len()));
        crate::lane::blocking(move || {
            Ok(transact(&this, root_id, description, move |batch| {
                let mut acts = Vec::new();
                let mut outcomes = Vec::new();
                for relocation in &copies {
                    let from = addressable(&relocation.from)?;
                    let to = addressable(&relocation.to)?;
                    // A copy into its own subtree would recurse forever
                    // — the same guard as move, and the reason it lives
                    // in one function.
                    guard_into_self(&from, &to)?;
                    let from_disk = batch.require_source(&from)?;
                    batch.require_parent(&to)?;

                    let Some((landed, displace)) = batch.settle(to, on_conflict)? else {
                        outcomes.push(Outcome {
                            path: from,
                            landed_at: None,
                            skipped: true,
                        });
                        continue;
                    };
                    let to_disk = batch.disk(&landed)?;
                    if displace {
                        acts.push(Act::Displace(to_disk.clone()));
                    }
                    acts.push(Act::Copy {
                        from: from_disk,
                        to: to_disk,
                    });
                    outcomes.push(Outcome {
                        path: from,
                        landed_at: Some(landed),
                        skipped: false,
                    });
                }
                Ok((acts, outcomes))
            }))
        })
        .await?
    }

    /// Delete a set of paths, atomically across the set.
    ///
    /// The checkpoint is the recovery mechanism — see the module doc on
    /// why there is no trash.
    // t[impl files.write.surface] — deletion is a checkpoint, not a trash move
    async fn delete_paths(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
    ) -> Result<WriteReceipt, FilesFault> {
        let this = self.clone();
        let description = format!("delete {} path{}", paths.len(), plural_s(paths.len()));
        crate::lane::blocking(move || {
            Ok(transact(&this, root_id, description, move |batch| {
                let mut acts = Vec::new();
                let mut outcomes = Vec::new();
                for path in &paths {
                    let path = addressable(path)?;
                    let disk = batch.require_source(&path)?;
                    acts.push(Act::Displace(disk));
                    outcomes.push(Outcome {
                        path,
                        landed_at: None,
                        skipped: false,
                    });
                }
                Ok((acts, outcomes))
            }))
        })
        .await?
    }

    /// Not reachable yet: the ticket has nowhere to be redeemed.
    ///
    /// A `ByteTicket` is a promise that the byte lane will honour a
    /// token, and this codebase has no byte lane. Minting one would hand
    /// every caller a token that fails at redemption time, in a different
    /// component, with no way to tell a bad ticket from a bad stream —
    /// so this refuses in the one place that knows why.
    // t[impl files.write.surface] — declared, and honestly unimplemented
    // t[impl files.write.surface] — a selection downloads as one stream
    /// Open an archive stream over a selection.
    ///
    /// The ticket names the paths, not their content: a tar is generated
    /// as it is sent, so nothing is materialised server-side and a
    /// selection of a whole root costs one buffer. That is also why it
    /// reports no length and refuses a range — the size is not known
    /// until the archive has been produced, and a stream generated in
    /// one pass cannot seek.
    ///
    /// Every path is validated and checked to exist before a token is
    /// minted, so a caller holding a ticket is not holding a promise
    /// about a path that was never there.
    async fn archive(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
    ) -> Result<ByteTicket, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        if paths.is_empty() {
            return Err(FilesFault::invalid("an archive needs at least one path"));
        }
        let mut checked = Vec::with_capacity(paths.len());
        for path in paths {
            let path = path.validate()?;
            let disk = std::path::Path::new(&root.path).join(path.as_str());
            if !disk.exists() {
                return Err(FilesFault::PathNotFound(path));
            }
            checked.push(path);
        }
        self.mint_archive(root_id, &checked)
    }
}

fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn plural_y(n: usize) -> &'static str {
    if n == 1 { "y" } else { "ies" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RootPath {
        RootPath::parse(s).expect("test path")
    }

    // t[verify files.write.surface]
    #[test]
    fn a_destination_inside_its_own_source_is_refused() {
        assert!(matches!(
            guard_into_self(&p("Sessions"), &p("Sessions/Archive")),
            Err(FilesFault::IntoSelf { .. })
        ));
        assert!(matches!(
            guard_into_self(&p("Sessions"), &p("Sessions")),
            Err(FilesFault::IntoSelf { .. })
        ));
        assert!(guard_into_self(&p("Sessions"), &p("Archive/Sessions")).is_ok());
        assert!(
            guard_into_self(&p("Session"), &p("Sessions/x")).is_ok(),
            "a shared prefix is not containment"
        );
    }

    #[test]
    fn the_root_itself_is_not_a_subject() {
        assert!(matches!(
            addressable(&RootPath::root()),
            Err(FilesFault::Invalid(_))
        ));
        // The wire type skips `parse`, so the guard has to be here.
        let hostile: RootPath =
            serde_json::from_str("\"../../etc/passwd\"").expect("transparent newtype");
        assert!(matches!(addressable(&hostile), Err(FilesFault::BadPath(_))));
    }
}
