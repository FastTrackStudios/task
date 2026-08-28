//! The org vault as a File Root — `project.vault.write-path`.
//!
//! The rule: "the production vault becomes a File Root and all writes
//! to it go through the Files API, markdown included. Its live tree
//! stays ordinary files on disk — greppable, `cp -r`-able, editable by
//! any other tool — so migration changes the write path and not the
//! on-disk result."
//!
//! Two halves, both here. [`FilesBackend::adopt_vault`] makes the vault
//! directory a root — in place, nothing moved, the same adoption any
//! folder gets — and then binds a [`VaultSink`] for it in `vault-live`'s
//! page-sink registry. From that moment every page any entity store in
//! the process writes (`VaultEntityStore::create`, `save_page`,
//! `delete_page`) arrives at [`FilesBackend::write_page_inner`] instead
//! of `std::fs`, and a write is what a write is anywhere else in Files:
//! atomic in the tree, heard by the catalogue as a delta, and an
//! activity hint into the cadence engine so the vault's history is kept
//! the way a session folder's is — snapshots on a timer, a checkpoint at
//! quiescence — rather than a commit per keystroke.
//!
//! # Why not a checkpoint per page
//!
//! `WriteService`'s structural verbs checkpoint per batch, because a
//! rename or a delete *is* the user's action and history should show it
//! whole. A page save is content, like an app saving a session file, and
//! content goes through the cadence — which is also what keeps this
//! cheap: a vault of ten thousand pages is not scanned ten thousand
//! times because ten thousand tasks were ticked off.
//!
//! # What a vault that is not a root gets
//!
//! Exactly what it got before: no sink bound, so `vault-live` writes to
//! the filesystem itself. A CLI that opens a vault without a Files
//! backend, a unit test with a temp dir — neither changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use files_proto::error::FilesFault;
use files_proto::id::RootId;
use files_proto::model::{FileRootInfo, RootFlavor};
use files_proto::path::RootPath;

use crate::backend::FilesBackend;

/// The name the vault is registered under when this backend adopts it.
pub const VAULT_ROOT_NAME: &str = "Vault";

/// One adopter per vault per process.
///
/// Keyed by canonical vault path; the value is the root it became and
/// the `data_dir` of the backend that holds it. Two backends in one
/// process pointed at one vault directory is a real configuration —
/// `TASK_SERVER_VAULT_ROOT` makes every org share one, for tests and
/// flat containers — and the second to arrive must not open a second
/// store over the first's `.fts-files`: two `FsStore`s over one on-disk
/// chunk store in one process is the shape that hangs (see
/// `tests/rpc_surface.rs`). So the second adopter finds the first here
/// and leaves the binding alone. [`FilesBackend::release_vault`] takes
/// the entry back out on shutdown, which is what lets a restart in the
/// same process adopt afresh.
fn adopted() -> &'static Mutex<HashMap<PathBuf, (RootId, PathBuf)>> {
    static ADOPTED: std::sync::OnceLock<Mutex<HashMap<PathBuf, (RootId, PathBuf)>>> =
        std::sync::OnceLock::new();
    ADOPTED.get_or_init(|| Mutex::new(HashMap::new()))
}

impl FilesBackend {
    // t[impl project.vault.write-path] — the vault becomes a File Root,
    // in place, and every page write in the process is routed through
    // this backend from here on
    /// Make this backend's vault a File Root and route every page write
    /// under it through the Files API.
    ///
    /// Idempotent: a vault that is already a root — from a previous boot,
    /// the marker in the directory says so — is re-bound, not
    /// re-registered. The directory is created when absent, because a
    /// fresh org has no vault yet and the rule is about what the vault
    /// *is*, not about whether anyone has written to it.
    ///
    /// # Errors
    ///
    /// Registration faults: the directory is not one, or the marker
    /// inside it names a root registered at another path (a vault that
    /// was moved — the same refusal any moved root gets, see
    /// `create_root_inner`).
    pub fn adopt_vault(&self) -> Result<RootId, FilesFault> {
        crate::backend::off_worker(|| self.adopt_vault_inner())
    }

    fn adopt_vault_inner(&self) -> Result<RootId, FilesFault> {
        let vault = self.vault_root().to_path_buf();
        std::fs::create_dir_all(&vault).map_err(FilesFault::io)?;
        let canonical = vault.canonicalize().map_err(FilesFault::io)?;

        // Held across the registration, so two backends arriving at once
        // cannot both find the vault unadopted.
        let mut held = adopted().lock().expect("adopted vaults poisoned");
        if let Some((root_id, holder)) = held.get(&canonical) {
            if *holder != self.data_dir_path() {
                tracing::warn!(
                    vault = %canonical.display(),
                    root_id = %root_id.get(),
                    "files: vault already adopted by another backend in this process; \
                     sharing its binding"
                );
            }
            return Ok(*root_id);
        }

        let root = match self
            .registry_list()
            .into_iter()
            .find(|r| r.local_tree().is_some_and(|tree| tree == canonical))
        {
            Some(known) => known,
            None => {
                self.register_root(canonical.clone(), VAULT_ROOT_NAME.into(), RootFlavor::Media)?
            }
        };
        let root_id = RootId::new(root.id);
        vault::bind_sink(
            &canonical,
            Arc::new(VaultSink {
                backend: self.clone(),
                root: root_id,
            }),
        );
        held.insert(canonical.clone(), (root_id, self.data_dir_path()));
        tracing::info!(root_id = %root.id, vault = %canonical.display(), "files: vault adopted as a root");
        Ok(root_id)
    }

    /// Undo [`FilesBackend::adopt_vault`]'s process-level half: unbind
    /// the sink and forget the adoption, so the next backend to open this
    /// vault — a restart in the same process — adopts it afresh. Called
    /// from `shutdown`. The root itself stays registered on disk; that is
    /// the point of the marker.
    pub(crate) fn release_vault(&self) {
        let Ok(canonical) = self.vault_root().canonicalize() else {
            return;
        };
        let mut held = adopted().lock().expect("adopted vaults poisoned");
        if held
            .get(&canonical)
            .is_some_and(|(_, holder)| *holder == self.data_dir_path())
        {
            held.remove(&canonical);
            vault::unbind_sink(&canonical);
        }
    }

    /// The root this backend's vault is registered as, if it has been
    /// adopted.
    #[must_use]
    pub fn vault_root_id(&self) -> Option<RootId> {
        let canonical = self.vault_root().canonicalize().ok()?;
        self.registry_list()
            .into_iter()
            .find(|r| r.local_tree().is_some_and(|tree| tree == canonical))
            .map(|r| RootId::new(r.id))
    }

    // t[impl project.vault.write-path] — a page write is a Files write:
    // atomic in the tree, a catalogue delta, a cadence hint
    // t[impl vault.write.atomic] — the same temp-and-rename, inside the
    // root's tree, under the root's write lock
    /// Write one page's bytes into a root's tree, the Files way.
    pub(crate) fn write_page_inner(
        &self,
        root_id: RootId,
        rel: &str,
        bytes: &[u8],
    ) -> Result<(), FilesFault> {
        let (root, path, disk) = self.page_location(root_id, rel)?;
        {
            // Serialised with the structural lane: a batch renaming this
            // page's folder must not interleave with a save into it.
            let lock = self.root_lock(root_id.get());
            let _guard = lock.lock().expect("root write lock poisoned");
            if let Some(parent) = disk.parent() {
                std::fs::create_dir_all(parent).map_err(FilesFault::io)?;
            }
            vault::write_atomic(&disk, bytes).map_err(FilesFault::Io)?;
        }
        self.page_written(&root, std::slice::from_ref(&path), &[]);
        Ok(())
    }

    /// Remove one page from a root's tree. Absent is not an error —
    /// `delete_page` is allowed to be told about a page twice.
    pub(crate) fn remove_page_inner(&self, root_id: RootId, rel: &str) -> Result<(), FilesFault> {
        let (root, path, disk) = self.page_location(root_id, rel)?;
        {
            let lock = self.root_lock(root_id.get());
            let _guard = lock.lock().expect("root write lock poisoned");
            match std::fs::remove_file(&disk) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(FilesFault::io(e)),
            }
        }
        self.page_written(&root, &[], std::slice::from_ref(&path));
        Ok(())
    }

    /// Move one page within a root's tree. The caller has already
    /// refused an occupied destination; under the lock it is checked
    /// again so the refusal holds under a race.
    pub(crate) fn move_page_inner(
        &self,
        root_id: RootId,
        from: &str,
        to: &str,
    ) -> Result<(), FilesFault> {
        let (root, from_path, src) = self.page_location(root_id, from)?;
        let (_, to_path, dst) = self.page_location(root_id, to)?;
        {
            let lock = self.root_lock(root_id.get());
            let _guard = lock.lock().expect("root write lock poisoned");
            if dst.exists() {
                return Err(FilesFault::Exists { path: to_path });
            }
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(FilesFault::io)?;
            }
            std::fs::rename(&src, &dst).map_err(FilesFault::io)?;
        }
        self.page_written(
            &root,
            std::slice::from_ref(&to_path),
            std::slice::from_ref(&from_path),
        );
        Ok(())
    }

    /// Resolve a page's relative path against the root's tree.
    ///
    /// Through [`RootPath::parse`], so a page path is held to the same
    /// rules as any other path in the lane — no `..`, no absolute, no
    /// empty segments — and a vault page can never name a file outside
    /// the vault.
    fn page_location(
        &self,
        root_id: RootId,
        rel: &str,
    ) -> Result<(FileRootInfo, RootPath, std::path::PathBuf), FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        let tree = crate::lane::lane_tree(&root)?.to_path_buf();
        let path = RootPath::parse(rel)?;
        let disk = tree.join(path.as_str());
        Ok((root, path, disk))
    }

    /// What follows any write to the tree: the catalogue hears about it
    /// as a delta, and the cadence engine hears there was activity.
    fn page_written(&self, root: &FileRootInfo, touched: &[RootPath], removed: &[RootPath]) {
        crate::lane::tree::note_write(self, root, touched, removed);
        let hinted: Vec<String> = touched
            .iter()
            .chain(removed)
            .map(|p| p.as_str().to_owned())
            .collect();
        if let Err(err) = self.hint_activity_inner(root.id, hinted) {
            tracing::warn!(root_id = %root.id, %err, "files: vault write not hinted to the cadence");
        }
    }
}

/// The `vault-live` page sink bound by [`FilesBackend::adopt_vault`].
struct VaultSink {
    backend: FilesBackend,
    root: RootId,
}

/// Every sink method runs through [`crate::backend::off_worker`]: a
/// page is saved from wherever the entity store was called — an RPC
/// handler inline on the runtime, the vault-sync lane, a CLI — and the
/// catalogue/cadence half of the write touches the root's store with
/// `pollster`, which must not happen on a runtime worker.
impl vault::PageSink for VaultSink {
    fn write(&self, _root: &Path, rel: &str, bytes: &[u8]) -> Result<(), String> {
        crate::backend::off_worker(|| self.backend.write_page_inner(self.root, rel, bytes))
            .map_err(|e| e.to_string())
    }

    fn remove(&self, _root: &Path, rel: &str) -> Result<(), String> {
        crate::backend::off_worker(|| self.backend.remove_page_inner(self.root, rel))
            .map_err(|e| e.to_string())
    }

    fn rename(&self, _root: &Path, from: &str, to: &str) -> Result<(), String> {
        crate::backend::off_worker(|| self.backend.move_page_inner(self.root, from, to))
            .map_err(|e| e.to_string())
    }
}
