//! `VaultSync` backend — the canonical filesystem implementation
//! of [`vault_proto::VaultSync`].
//!
//! One [`Backend`] instance serves one-or-more vault roots, each
//! addressed by an opaque `vault_id`. A desktop app typically has
//! one backend with one vault registered; a server can register
//! many. Both shapes go through the same wire trait + the same
//! `architect::serve` mount point, so a remote client doesn't
//! know which backend it's talking to.
//!
//! Conflict policy: **last-writer-wins with `IfMatch`**, mirroring
//! the proto:
//! - [`IfMatch::CreateOnly`] — fail if the file exists.
//! - [`IfMatch::Sha`]        — fail unless the server's current
//!   sha matches.
//! - [`IfMatch::Force`]      — unconditional. Only safe on the
//!   first push of a brand-new vault.
//!
//! Live events: every successful PUT / DELETE goes to two places
//! at once (see [`Backend::emit`]) — the per-vault
//! `broadcast::Sender` that in-process listeners (collab
//! write-behind, the server's link-sync loop) hold, and the
//! `architect::PubSub` hub behind the `#[subscribe] fn changes`
//! stream that wire subscribers attach to. Wire events are wrapped
//! as [`VaultChange`] so the `vault_id` travels with them — the
//! stream is unfiltered and clients keep the vault they browse.
//!
//! Disk-side externalities (file changes from outside the
//! backend — vim/obsidian/git pulls/etc.) are picked up by
//! attaching a watcher via [`Backend::start_watcher`]: the
//! returned [`WatcherHandle`] keeps a [`crate::watcher`] alive
//! and forwards every FS change onto both of the above. Drop the
//! handle to detach. Caller is expected to start one watcher per
//! registered vault; the backend itself doesn't auto-spawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;
use vault_proto::{
    BaseGroup, BaseRowView, BaseView, CollabAck, FileBytes, FolderIndex, IfMatch, Manifest,
    ManifestEntry, MoveAck, PageMeta, PutAck, TextHit, VaultChange, VaultEvent, VaultSync,
    VaultSyncError, VaultSyncStreamSource, collab_doc_id,
};

use crate::vault::Vault;
use crate::watcher::{self, WatchError};
use crate::write_guard::{WriteGuard, current_caller};
use editor_state::markdown::{FrontMatter, PropValue, parse_frontmatter};

/// Debounce window for the FS watcher attached by
/// [`Backend::start_watcher`]. Coalesces editor swap-file dances
/// + git-pull bursts into one event per touched path.
const WATCHER_DEBOUNCE: Duration = Duration::from_millis(500);

/// Handle for a per-vault filesystem watcher attached via
/// [`Backend::start_watcher`]. Keeps the watcher + the
/// forwarding thread alive; drop to stop receiving external
/// disk events.
pub struct WatcherHandle {
    _guard: watcher::WatcherGuard,
}

/// How the backend resolves `vault_id` → on-disk path.
#[derive(Debug, Clone)]
enum Layout {
    /// Each `vault_id` maps to an explicit absolute path.
    /// Unknown ids return [`VaultSyncError::NotFound`]. Right
    /// for a desktop app that opens a finite, user-chosen set
    /// of vaults.
    ///
    /// Behind a lock because the set can grow while the backend
    /// serves ([`Backend::add_root`] — a wiki created at runtime
    /// becomes a root without a restart), and every clone must
    /// see the addition.
    Explicit(Arc<std::sync::RwLock<HashMap<String, PathBuf>>>),
    /// All vaults live as subdirectories under one parent
    /// directory. Any `vault_id` is accepted; the directory is
    /// created on the first write. Right for a multi-tenant
    /// server hosting many client vaults.
    UnderParent(PathBuf),
}

/// Filesystem-backed `VaultSync` implementation. Cheap to
/// `Clone` — internals are `Arc`d.
///
/// Two layouts (pick one per backend instance):
/// - [`Backend::single`] / [`Backend::with_roots`]: explicit
///   `vault_id → path` registry. Unknown ids fail.
/// - [`Backend::under_parent`]: open-ended, one subdir per
///   vault under a shared parent. Unknown ids auto-create.
#[derive(Clone, architect::HasDispatcher)]
#[dispatch(crate::write_guard::CallerDispatcher)]
pub struct Backend {
    layout: Layout,
    /// Coarse global write lock. Reads bypass it; writes
    /// `lock` → `read-sha` → `write` → `unlock`. Refine to a
    /// per-vault `RwLock` if write contention shows up.
    write_lock: Arc<std::sync::Mutex<()>>,
    /// Per-vault broadcast sender, lazily created on first use.
    /// Capacity 256: rapid bursts coalesce client-side via
    /// [`VaultEvent::Resync`].
    ///
    /// In-process fan-out only (the collab write-behind, the
    /// server's link-sync loop). Wire subscribers ride the
    /// [`Self::changes`] hub below.
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<VaultEvent>>>>,
    /// Fan-out hub behind the `#[subscribe] fn changes` stream —
    /// every event that goes onto a per-vault broadcast channel is
    /// published here too, wrapped with its `vault_id` so
    /// subscribers (who see *all* vaults) can filter. Sliding
    /// mailbox: a slow subscriber loses its oldest queued events
    /// and re-pulls when its stream re-establishes. Clones share
    /// the hub (`Arc` inside), so the service mount and the stream
    /// mount can each hold a backend clone.
    changes: architect::PubSub<VaultChange>,
    /// CRDT collaboration registrations: `doc_id → (vault_id, path)`.
    /// Populated by [`VaultSync::open_collab`]; consulted by the
    /// server's doc-registry admission hook (which only sees a
    /// `Uuid`) and by the write-behind / inbound reconciler to route
    /// file events to open docs. `std::sync::RwLock` — holds are
    /// instant lookups from both sync (dispatcher blocking pool) and
    /// async contexts.
    collab: Arc<std::sync::RwLock<HashMap<Uuid, (String, String)>>>,
    /// Extra roots scanned for `.cook` recipes when building base rows:
    /// `vault_id → root`. Recipes live under the wiki root, outside the
    /// vault this backend serves, so without this a `.base` filtering
    /// `type: recipe` matches nothing. Read-only and used by
    /// [`VaultSync::base_views`] alone — recipes are not part of the
    /// manifest, are never synced through here, and stay owned by the
    /// `cookbook` service. Empty by default.
    recipe_roots: Arc<HashMap<String, PathBuf>>,
    /// Asked before every caller write — see [`crate::write_guard`].
    /// Unset allows everything, which is what a desktop vault wants.
    /// Shared across clones, like the roots: the server hands clones of
    /// this backend to the collab and shelf machinery before the wiki
    /// backend that answers for wikis exists.
    guard: Arc<std::sync::OnceLock<Arc<dyn WriteGuard>>>,
}

impl Backend {
    /// Build a backend serving a single vault under `root` as
    /// `vault_id`. The directory is created if missing.
    pub fn single(vault_id: impl Into<String>, root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&root)?;
        let mut roots = HashMap::with_capacity(1);
        roots.insert(vault_id.into(), root);
        Ok(Self::with_roots(roots))
    }

    /// Build a backend from a pre-built `vault_id → root` map.
    /// Caller is responsible for creating the directories.
    #[must_use]
    pub fn with_roots(roots: HashMap<String, PathBuf>) -> Self {
        Self::from_layout(Layout::Explicit(Arc::new(std::sync::RwLock::new(roots))))
    }

    /// Register one more `vault_id → root` on an explicit-layout
    /// backend, creating the directory if missing. Every clone sees
    /// it at once — this is how a wiki created while the server runs
    /// becomes editable through the vault path without a restart.
    /// Replaces an existing entry of the same id. An `UnderParent`
    /// backend needs no registration and ignores the call.
    pub fn add_root(&self, vault_id: impl Into<String>, root: PathBuf) -> std::io::Result<()> {
        if let Layout::Explicit(map) = &self.layout {
            std::fs::create_dir_all(&root)?;
            map.write()
                .expect("vault::sync roots poisoned")
                .insert(vault_id.into(), root);
        }
        Ok(())
    }

    /// The registered `vault_id`s (explicit layout only; an
    /// `UnderParent` backend accepts any id and lists none).
    #[must_use]
    pub fn vault_ids(&self) -> Vec<String> {
        match &self.layout {
            Layout::Explicit(map) => {
                let mut ids: Vec<String> = map
                    .read()
                    .expect("vault::sync roots poisoned")
                    .keys()
                    .cloned()
                    .collect();
                ids.sort();
                ids
            }
            Layout::UnderParent(_) => Vec::new(),
        }
    }

    /// Build a multi-tenant backend where every `vault_id`
    /// resolves to `{parent}/{vault_id}/`. Subdirs are created
    /// on demand on the first write. `parent` itself is
    /// created up front.
    pub fn under_parent(parent: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&parent)?;
        Ok(Self::from_layout(Layout::UnderParent(parent)))
    }

    fn from_layout(layout: Layout) -> Self {
        Self {
            layout,
            write_lock: Arc::new(std::sync::Mutex::new(())),
            channels: Arc::new(RwLock::new(HashMap::new())),
            changes: architect::PubSub::sliding(256),
            collab: Arc::new(std::sync::RwLock::new(HashMap::new())),
            recipe_roots: Arc::new(HashMap::new()),
            guard: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Register the roots holding each vault's `.cook` recipes — in
    /// practice the org's wiki root, whose `Cookbook/` subtree the
    /// `cookbook` service owns. Recipes found there become base rows
    /// stamped `type: recipe`, so a `.base` can list and filter the
    /// cookbook alongside ordinary notes. Without this, recipes are
    /// invisible to bases; nothing else about the backend changes.
    #[must_use]
    pub fn with_recipe_roots(mut self, roots: HashMap<String, PathBuf>) -> Self {
        self.recipe_roots = Arc::new(roots);
        self
    }

    /// Ask `guard` before every write a caller makes. Every clone sees
    /// it at once. Set once; a second call is ignored and returns
    /// `false`, since a guard swapped under running writes would make
    /// the rule depend on timing.
    pub fn set_write_guard(&self, guard: Arc<dyn WriteGuard>) -> bool {
        self.guard.set(guard).is_ok()
    }

    /// After `from` moved to `to`: when a markdown page's name changed,
    /// rewrite every wikilink in the vault that named it (see
    /// [`crate::relink`]). Each rewrite is a committed write under the
    /// same guard as any other, and announced, so an open document
    /// merges it in. Returns the pages rewritten.
    fn relink_after_move(
        &self,
        vault_id: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<String>, VaultSyncError> {
        use crate::relink::{PageName, relink};
        let is_md = |p: &str| p.to_ascii_lowercase().ends_with(".md");
        let (old, new) = (PageName::of(from), PageName::of(to));
        if !is_md(from) || !is_md(to) || old == new {
            return Ok(Vec::new());
        }
        let mut relinked = Vec::new();
        for entry in self.manifest(vault_id)?.files {
            if !is_md(&entry.path) {
                continue;
            }
            let abs = self.file_path(vault_id, &entry.path)?;
            let Ok(text) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let Some(rewritten) = relink(&text, &old, &new) else {
                continue;
            };
            // A page this person may not write keeps its link, and the
            // move still stands: the link now dangles, which is visible
            // and fixable, where refusing the whole move would not be.
            if self.guard_write(vault_id, &entry.path).is_err() {
                continue;
            }
            let g = self
                .write_lock
                .lock()
                .expect("vault::sync write_lock poisoned");
            // Written only if nobody changed it since it was read.
            let current = std::fs::read_to_string(&abs).unwrap_or_default();
            if current != text {
                continue;
            }
            let bytes = rewritten.into_bytes();
            let (sha256, mtime_ms) = write_file_atomic(&abs, &bytes)?;
            drop(g);
            self.emit(
                vault_id,
                VaultEvent::Put {
                    path: entry.path.clone(),
                    sha256,
                    mtime_ms,
                    size: bytes.len() as u64,
                },
            );
            relinked.push(entry.path);
        }
        Ok(relinked)
    }

    /// May the caller running on this thread write `path`?
    fn guard_write(&self, vault_id: &str, path: &str) -> Result<(), VaultSyncError> {
        self.check_write(vault_id, path, current_caller().as_deref())
    }

    /// Would `principal` be allowed to write `path`? For a host that
    /// reaches a file some other way — the collab sync lane, which
    /// knows a doc id rather than a path — and must apply the same rule.
    pub fn check_write(
        &self,
        vault_id: &str,
        path: &str,
        principal: Option<&str>,
    ) -> Result<(), VaultSyncError> {
        match self.guard.get() {
            Some(guard) => guard
                .check(vault_id, path, principal)
                .map_err(VaultSyncError::Refused),
            None => Ok(()),
        }
    }

    /// Announce a committed change: onto `vault_id`'s in-process
    /// broadcast channel AND the wire hub. Call only *after* the
    /// write landed on disk — subscribers fold these into state they
    /// fetched via `manifest` / `folder_index`, so a speculative
    /// event would desync them.
    fn emit(&self, vault_id: &str, event: VaultEvent) {
        let _ = self.channel_blocking(vault_id).send(event.clone());
        self.changes.publish(VaultChange {
            vault_id: vault_id.to_string(),
            event,
        });
    }

    /// Reverse-resolve a collab doc id to its `(vault_id, path)`,
    /// if [`VaultSync::open_collab`] registered it. This is what the
    /// doc registry's admission hook calls — an unregistered id is
    /// not served.
    #[must_use]
    pub fn collab_route(&self, doc_id: Uuid) -> Option<(String, String)> {
        self.collab
            .read()
            .expect("vault::sync collab map poisoned")
            .get(&doc_id)
            .cloned()
    }

    /// Register `path` in `vault_id` for collaboration and return its doc
    /// id — [`VaultSync::open_collab`] without the caller's write guard,
    /// for a lane that is gated on its own: the peer document lane
    /// (`offline.peers`), where the caller is an admitted node rather than
    /// a person and the rule is "a peer exchanges documents". The path is
    /// validated as for every other call; a path with no file registers
    /// too (the session that follows makes the document, and the
    /// write-behind the file).
    ///
    /// # Errors
    ///
    /// The vault is unknown, or the path could leave it.
    pub fn register_collab(&self, vault_id: &str, path: &str) -> Result<Uuid, VaultSyncError> {
        self.file_path(vault_id, path)?;
        let doc_id = collab_doc_id(vault_id, path);
        self.collab
            .write()
            .expect("vault::sync collab map poisoned")
            .insert(doc_id, (vault_id.to_string(), path.to_string()));
        Ok(doc_id)
    }

    /// Resolve `vault_id` to an absolute root path. Returns
    /// [`VaultSyncError::NotFound`] only in `Explicit` mode
    /// when the id is unregistered; `UnderParent` always
    /// succeeds.
    ///
    /// Public because a server-side lane that keeps its documents in a
    /// vault has to *read* them by walking — the Assets tier's chart
    /// lane parses frontmatter off every chart to list them, and no
    /// wire call returns parsed frontmatter for a subtree. Writes still
    /// go through [`VaultSync::put_file`], which is the part that must
    /// not be bypassed: it takes the write lock, hashes, and broadcasts
    /// the `Put` that the collab write-behind and the FS watcher key
    /// off. Reading around the backend is safe; writing around it is
    /// not.
    pub fn root(&self, vault_id: &str) -> Result<PathBuf, VaultSyncError> {
        match &self.layout {
            Layout::Explicit(map) => map
                .read()
                .expect("vault::sync roots poisoned")
                .get(vault_id)
                .cloned()
                .ok_or(VaultSyncError::NotFound),
            Layout::UnderParent(parent) => Ok(parent.join(vault_id)),
        }
    }

    /// The manifest's rows for only the files `keep` admits (by
    /// root-relative path), hashing nothing else.
    ///
    /// [`VaultSync::manifest`] hashes every file in the vault. A caller
    /// that wants a subset — the doc peer lists only markdown — would
    /// otherwise stream every attachment, PDF and bounce through SHA-256
    /// to throw the row away.
    ///
    /// # Errors
    ///
    /// As [`VaultSync::manifest`].
    pub fn manifest_where(
        &self,
        vault_id: &str,
        keep: impl Fn(&str) -> bool,
    ) -> Result<Manifest, VaultSyncError> {
        let dir = self.root(vault_id)?;
        let mut files = Vec::new();
        if dir.exists() {
            collect(&dir, &dir, &keep, &mut files)?;
        }
        Ok(Manifest {
            vault_id: vault_id.to_string(),
            files,
        })
    }

    /// The root-relative path of every file [`VaultSync::manifest`]
    /// would list, without reading or hashing any of them — for a caller
    /// that only matches on names (`search_vault`).
    ///
    /// # Errors
    ///
    /// The vault is unknown, or its directory cannot be read.
    pub fn paths(&self, vault_id: &str) -> Result<Vec<String>, VaultSyncError> {
        let dir = self.root(vault_id)?;
        let mut out = Vec::new();
        if dir.exists() {
            walk_files(&dir, &dir, &mut |_, rel| {
                out.push(rel);
                Ok(())
            })?;
        }
        Ok(out)
    }

    fn file_path(&self, vault_id: &str, rel: &str) -> Result<PathBuf, VaultSyncError> {
        // Refuse anything that could traverse out of the vault
        // dir. Reject `..`, absolute paths, and Windows drive
        // letters defensively.
        if rel.is_empty()
            || rel.starts_with('/')
            || rel.starts_with('\\')
            || rel.contains("..")
            || rel.contains(':')
        {
            return Err(VaultSyncError::BadPath);
        }
        let root = self.root(vault_id)?;
        // Out of the manifest is not enough: a private file must not be
        // readable — or writable — by spelling its path either.
        // t[impl vault.private.node-only]
        if files_proto::consts::is_within_private(&root, rel) {
            return Err(VaultSyncError::NotFound);
        }
        Ok(root.join(rel))
    }

    /// Get-or-create the per-vault broadcast sender. Async —
    /// uses tokio's `RwLock` so it interoperates with the async
    /// `subscribe` impl.
    pub async fn channel(&self, vault_id: &str) -> broadcast::Sender<VaultEvent> {
        if let Some(tx) = self.channels.read().await.get(vault_id) {
            return tx.clone();
        }
        let mut chans = self.channels.write().await;
        if let Some(tx) = chans.get(vault_id) {
            return tx.clone();
        }
        let (tx, _rx) = broadcast::channel::<VaultEvent>(256);
        chans.insert(vault_id.to_string(), tx.clone());
        tx
    }

    /// Attach a debounced filesystem watcher to `vault_id`.
    /// External edits (vim, Obsidian, `git pull`, …) under the
    /// vault root are translated into the same
    /// [`vault_proto::VaultEvent`]s that PUT/DELETE wire calls
    /// emit, and pushed onto the broadcast channel `subscribe`
    /// listens to.
    ///
    /// The forwarder runs on a dedicated OS thread. Dropping the
    /// returned [`WatcherHandle`] closes the underlying
    /// debouncer; the thread exits naturally on the next loop
    /// iteration as the sender side hangs up.
    ///
    /// Caveats:
    /// - Subscribers may observe duplicates after their own
    ///   writes (the broadcast emits once on commit, the
    ///   watcher emits again on the disk event). The duplicate
    ///   carries the same `sha256`, so clients can dedupe on
    ///   that.
    /// - Only `Explicit` / `UnderParent`-registered `vault_ids`
    ///   resolve. The watcher fails up front for unknown ids.
    /// - For `UnderParent` layouts the root dir must already
    ///   exist (which it always does after the first write); to
    ///   pre-attach before any write, create the subdir first.
    pub async fn start_watcher(&self, vault_id: &str) -> Result<WatcherHandle, WatchError> {
        let root = self
            .root(vault_id)
            .map_err(|_| WatchError::Notify(format!("unknown vault id `{vault_id}`")))?;
        let tx = self.channel(vault_id).await;
        // On a thread of its own: the watch registers with the OS's file
        // event service (FSEvents on macOS), which can take seconds to
        // answer on a busy machine — seconds an async worker would spend
        // blocked, stalling every task scheduled on it. Not the blocking
        // pool either: a registration cannot be cancelled, and a runtime
        // shutting down waits for every blocking task, so one stuck in
        // `fseventsd` would hold the process open. A detached thread is
        // simply abandoned at exit.
        let watched = root.clone();
        let (done, started) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name(format!("vault-watch-start:{vault_id}"))
            .spawn(move || {
                let _ = done.send(watcher::watch(watched, WATCHER_DEBOUNCE));
            })
            .map_err(|e| WatchError::Notify(format!("spawn watch thread: {e}")))?;
        let (rx, guard) = started
            .await
            .map_err(|e| WatchError::Notify(format!("watch thread: {e}")))??;

        let thread_name = format!("vault-sync-watcher:{vault_id}");
        let hub = self.changes.clone();
        let id = vault_id.to_string();
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || forward_watcher_events(root, rx, tx, hub, id))
            .map_err(|e| WatchError::Notify(format!("spawn watcher thread: {e}")))?;

        Ok(WatcherHandle { _guard: guard })
    }

    /// [`Self::start_watcher`] for several vaults at once, over **one**
    /// OS watch ([`watcher::watch_many`]): one `fseventsd` registration
    /// for the set instead of one per vault. Events reach each vault's
    /// channel exactly as with a watcher of its own.
    ///
    /// # Errors
    ///
    /// An id is unknown, or no root could be watched.
    pub async fn start_watchers(&self, vault_ids: &[String]) -> Result<WatcherHandle, WatchError> {
        let mut roots = Vec::with_capacity(vault_ids.len());
        let mut channels = HashMap::with_capacity(vault_ids.len());
        for id in vault_ids {
            let root = self
                .root(id)
                .map_err(|_| WatchError::Notify(format!("unknown vault id `{id}`")))?;
            channels.insert(root.clone(), (id.clone(), self.channel(id).await));
            roots.push(root);
        }
        // A detached thread, for the reasons `start_watcher` gives.
        let (done, started) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("vault-watch-start".into())
            .spawn(move || {
                let _ = done.send(watcher::watch_many(roots, WATCHER_DEBOUNCE));
            })
            .map_err(|e| WatchError::Notify(format!("spawn watch thread: {e}")))?;
        let (receivers, guard) = started
            .await
            .map_err(|e| WatchError::Notify(format!("watch thread: {e}")))??;

        for (root, rx) in receivers {
            let Some((id, tx)) = channels.remove(&root) else {
                continue;
            };
            let hub = self.changes.clone();
            std::thread::Builder::new()
                .name(format!("vault-sync-watcher:{id}"))
                .spawn(move || forward_watcher_events(root, rx, tx, hub, id))
                .map_err(|e| WatchError::Notify(format!("spawn watcher thread: {e}")))?;
        }
        Ok(WatcherHandle { _guard: guard })
    }

    /// Sync sibling of [`Self::channel`] for use inside the
    /// sync trait methods. `TokioBlockingDispatcher` already
    /// runs us on a blocking-pool thread, so the
    /// `blocking_read`/`blocking_write` variants are safe.
    fn channel_blocking(&self, vault_id: &str) -> broadcast::Sender<VaultEvent> {
        if let Some(tx) = self.channels.blocking_read().get(vault_id) {
            return tx.clone();
        }
        let mut chans = self.channels.blocking_write();
        if let Some(tx) = chans.get(vault_id) {
            return tx.clone();
        }
        let (tx, _rx) = broadcast::channel::<VaultEvent>(256);
        chans.insert(vault_id.to_string(), tx.clone());
        tx
    }
}

impl VaultSync for Backend {
    fn manifest(&self, vault_id: &str) -> Result<Manifest, VaultSyncError> {
        let dir = self.root(vault_id)?;
        if !dir.exists() {
            return Ok(Manifest {
                vault_id: vault_id.to_string(),
                files: Vec::new(),
            });
        }
        let mut entries = Vec::new();
        collect(&dir, &dir, &|_| true, &mut entries)?;
        Ok(Manifest {
            vault_id: vault_id.to_string(),
            files: entries,
        })
    }

    fn get_file(&self, vault_id: &str, path: &str) -> Result<FileBytes, VaultSyncError> {
        let abs = self.file_path(vault_id, path)?;
        if !abs.exists() {
            return Err(VaultSyncError::NotFound);
        }
        let bytes = std::fs::read(&abs).map_err(io_err)?;
        Ok(FileBytes(bytes))
    }

    fn put_file(
        &self,
        vault_id: &str,
        path: &str,
        bytes: Vec<u8>,
        if_match: IfMatch,
    ) -> Result<PutAck, VaultSyncError> {
        let abs = self.file_path(vault_id, path)?;
        self.guard_write(vault_id, path)?;
        let g = self
            .write_lock
            .lock()
            .expect("vault::sync write_lock poisoned");
        let existing_sha = if abs.exists() {
            Some(sha256_file(&abs).map_err(io_err)?.0)
        } else {
            None
        };
        match (&if_match, existing_sha.as_deref()) {
            (IfMatch::CreateOnly, Some(_)) => return Err(conflict(&abs)),
            (IfMatch::Sha(want), Some(have)) if want != have => return Err(conflict(&abs)),
            _ => {}
        }
        let (new_sha, mtime_ms) = write_file_atomic(&abs, &bytes)?;
        drop(g);
        self.emit(
            vault_id,
            VaultEvent::Put {
                path: path.to_string(),
                sha256: new_sha.clone(),
                mtime_ms,
                size: bytes.len() as u64,
            },
        );
        Ok(PutAck {
            sha256: new_sha,
            mtime_ms,
        })
    }

    fn delete_file(
        &self,
        vault_id: &str,
        path: &str,
        if_match: IfMatch,
    ) -> Result<(), VaultSyncError> {
        let abs = self.file_path(vault_id, path)?;
        self.guard_write(vault_id, path)?;
        let g = self
            .write_lock
            .lock()
            .expect("vault::sync write_lock poisoned");
        if !abs.exists() {
            // Idempotent: missing path = success, no broadcast.
            return Ok(());
        }
        if let IfMatch::Sha(want) = &if_match {
            let (have, _) = sha256_file(&abs).map_err(io_err)?;
            if *want != have {
                return Err(conflict(&abs));
            }
        }
        std::fs::remove_file(&abs).map_err(io_err)?;
        prune_empty_parents(&self.root(vault_id)?, &abs);
        drop(g);
        self.emit(
            vault_id,
            VaultEvent::Delete {
                path: path.to_string(),
            },
        );
        Ok(())
    }

    fn move_file(
        &self,
        vault_id: &str,
        from: &str,
        to: &str,
        if_match: IfMatch,
    ) -> Result<MoveAck, VaultSyncError> {
        let src = self.file_path(vault_id, from)?;
        let dst = self.file_path(vault_id, to)?;
        self.guard_write(vault_id, from)?;
        self.guard_write(vault_id, to)?;
        let g = self
            .write_lock
            .lock()
            .expect("vault::sync write_lock poisoned");
        if !src.exists() {
            return Err(VaultSyncError::NotFound);
        }
        let bytes = std::fs::read(&src).map_err(io_err)?;
        let have = sha256_hex(&bytes);
        if let IfMatch::Sha(want) = &if_match
            && *want != have
        {
            return Err(VaultSyncError::Conflict {
                server_sha: have,
                server_bytes: bytes,
            });
        }
        if from == to {
            return Ok(MoveAck {
                sha256: have,
                relinked: Vec::new(),
            });
        }
        // A move never overwrites — the same promise as CreateOnly. A
        // case-only rename on a case-insensitive filesystem finds the
        // source here, which is the one "existing" file it may replace.
        if dst.exists() && !from.eq_ignore_ascii_case(to) {
            let existing = std::fs::read(&dst).map_err(io_err)?;
            return Err(VaultSyncError::Conflict {
                server_sha: sha256_hex(&existing),
                server_bytes: existing,
            });
        }
        let (sha256, mtime_ms) = write_file_atomic(&dst, &bytes)?;
        if !src.eq(&dst) {
            std::fs::remove_file(&src).map_err(io_err)?;
            prune_empty_parents(&self.root(vault_id)?, &src);
        }
        drop(g);
        self.emit(
            vault_id,
            VaultEvent::Delete {
                path: from.to_string(),
            },
        );
        self.emit(
            vault_id,
            VaultEvent::Put {
                path: to.to_string(),
                sha256: sha256.clone(),
                mtime_ms,
                size: bytes.len() as u64,
            },
        );
        let relinked = self.relink_after_move(vault_id, from, to)?;
        Ok(MoveAck { sha256, relinked })
    }

    fn folder_index(&self, vault_id: &str) -> Result<FolderIndex, VaultSyncError> {
        let dir = self.root(vault_id)?;
        if !dir.exists() {
            return Ok(FolderIndex {
                vault_id: vault_id.to_string(),
                pages: Vec::new(),
            });
        }
        // `Vault::open` walks every `.md` page and hands back the
        // raw bytes + derived basename, so we parse frontmatter
        // once here rather than re-walking the tree.
        let vault = Vault::open(&dir).map_err(|e| VaultSyncError::Internal(e.to_string()))?;
        let pages = vault
            .pages
            .iter()
            .map(|p| {
                let fm = parse_frontmatter(&p.raw);
                let get = |key: &str| fm.as_ref().and_then(|f| fm_text(f, key));
                PageMeta {
                    path: p.rel_path.clone(),
                    basename: p.basename.clone(),
                    title: page_title(&p.raw, &p.basename, get("title")),
                    page_type: get("type").unwrap_or_default(),
                    // `folder` or `up` — both wikilink-to-parent
                    // properties (the obsidian-virt-folder model).
                    folder: get("folder")
                        .or_else(|| get("up"))
                        .map(|s| strip_wikilink(&s))
                        .unwrap_or_default(),
                    // `raw` is the file's verbatim UTF-8 bytes, so this
                    // matches the manifest's per-file hash.
                    sha256: sha256_hex(p.raw.as_bytes()),
                    tags: fm.as_ref().map(fm_tags).unwrap_or_default(),
                    icon: get("icon").unwrap_or_default(),
                    aliases: fm.as_ref().map(fm_aliases).unwrap_or_default(),
                }
            })
            .collect::<Vec<PageMeta>>();
        let mut pages = pages;
        // `.base` view files are first-class vault citizens
        // (vault views): they appear in the folder index so
        // the tree shows them and deep links resolve. Title = the
        // basename; `page_type: "base"` lets clients badge them.
        for b in &vault.bases {
            // A base carries its own `folder:` / `tags:` (top-level YAML
            // keys parsed into `ParsedBase`), so it lands in the folder
            // tree + tags sidebar like a page. Fall back to empty when
            // the base failed to parse.
            let (folder, tags) = match &b.parsed {
                Ok(pb) => (pb.folder.clone(), pb.tags.clone()),
                Err(_) => (String::new(), Vec::new()),
            };
            pages.push(PageMeta {
                path: b.rel_path.clone(),
                basename: b.basename.clone(),
                title: b.basename.clone(),
                page_type: "base".to_string(),
                folder,
                tags,
                icon: String::new(),
                sha256: sha256_hex(b.raw.as_bytes()),
                aliases: Vec::new(),
            });
        }
        Ok(FolderIndex {
            vault_id: vault_id.to_string(),
            pages,
        })
    }

    fn base_views(&self, vault_id: &str, base_path: &str) -> Result<Vec<BaseView>, VaultSyncError> {
        let dir = self.root(vault_id)?;
        let vault = Vault::open(&dir).map_err(|e| VaultSyncError::Internal(e.to_string()))?;

        // Find + resolve the requested base.
        let parsed = vault
            .bases
            .iter()
            .find(|b| b.rel_path == base_path)
            .ok_or(VaultSyncError::NotFound)?
            .parsed
            .as_ref()
            .map_err(|e| VaultSyncError::Internal(format!("base parse: {e}")))?;

        // Every page → an executor row (frontmatter parsed once).
        let mut rows: Vec<crate::bases::BaseRow> = vault
            .pages
            .iter()
            .map(|p| {
                let fm_json = frontmatter_json(&p.raw);
                let ext = p.rel_path.rsplit_once('.').map_or("", |(_, e)| e);
                crate::bases::BaseRow::from_parts_full(
                    Uuid::new_v4(),
                    &p.basename,
                    &p.rel_path,
                    &p.folder,
                    ext,
                    &fm_json,
                    &[],
                )
            })
            .collect();

        // …plus the cookbook, which lives outside this vault root. Rows
        // keep the `.cook` extension so the client can route a click to
        // cook mode instead of the note viewer.
        if let Some(recipe_root) = self.recipe_roots.get(vault_id) {
            rows.extend(
                crate::cook::scan_cook_files(recipe_root)
                    .into_iter()
                    .map(|c| {
                        crate::bases::BaseRow::from_parts_full(
                            Uuid::new_v4(),
                            &c.basename,
                            &c.rel_path,
                            &c.folder,
                            "cook",
                            &crate::cook::cook_frontmatter_json(&c.raw),
                            &[],
                        )
                    }),
            );
        }

        // Run + project each view.
        let views = parsed
            .views
            .iter()
            .map(|v| {
                let executed = crate::bases::execute_view(parsed, v, rows.clone());
                let columns = v.order.clone();
                let groups = executed
                    .groups
                    .into_iter()
                    .map(|(label, group_rows)| BaseGroup {
                        label,
                        rows: group_rows
                            .iter()
                            .map(|r| BaseRowView {
                                path: r.path.clone(),
                                basename: r.basename.clone(),
                                title: r
                                    .frontmatter
                                    .get("title")
                                    .and_then(serde_json::Value::as_str)
                                    .map_or_else(|| r.basename.clone(), str::to_string),
                                cells: columns
                                    .iter()
                                    .map(|c| crate::bases::cell_value(r, c))
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect();
                BaseView {
                    name: v.name.clone(),
                    view_type: v.kind.as_str().to_string(),
                    columns,
                    groups,
                }
            })
            .collect();
        Ok(views)
    }

    fn search_text(
        &self,
        vault_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<TextHit>, VaultSyncError> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let dir = self.root(vault_id)?;
        if words.is_empty() || !dir.exists() {
            return Ok(Vec::new());
        }
        let vault = Vault::open(&dir).map_err(|e| VaultSyncError::Internal(e.to_string()))?;
        let mut hits: Vec<(bool, TextHit)> = vault
            .pages
            .iter()
            .filter_map(|p| {
                let fm = parse_frontmatter(&p.raw);
                let title = page_title(
                    &p.raw,
                    &p.basename,
                    fm.as_ref().and_then(|f| fm_text(f, "title")),
                );
                search_page(&p.raw, &p.rel_path, title, &words)
            })
            .collect();
        // Title matches first, then the most matching lines, then by name.
        hits.sort_by(|(at, a), (bt, b)| {
            bt.cmp(at)
                .then(b.matches.cmp(&a.matches))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        hits.truncate(limit.clamp(1, 200) as usize);
        Ok(hits.into_iter().map(|(_, h)| h).collect())
    }

    fn set_folder(
        &self,
        vault_id: &str,
        path: &str,
        parent: Option<String>,
        if_match: IfMatch,
    ) -> Result<PutAck, VaultSyncError> {
        let abs = self.file_path(vault_id, path)?;
        self.guard_write(vault_id, path)?;
        let g = self
            .write_lock
            .lock()
            .expect("vault::sync write_lock poisoned");
        if !abs.exists() {
            return Err(VaultSyncError::NotFound);
        }
        let bytes = std::fs::read(&abs).map_err(io_err)?;
        let existing_sha = sha256_hex(&bytes);
        if let IfMatch::Sha(want) = &if_match {
            if *want != existing_sha {
                return Err(VaultSyncError::Conflict {
                    server_sha: existing_sha,
                    server_bytes: bytes,
                });
            }
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| VaultSyncError::Internal("page is not valid UTF-8".into()))?;
        let mtime_now = || {
            std::fs::metadata(&abs)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64)
        };
        let Some(new_content) = apply_folder(&content, parent.as_deref()) else {
            // Already in the requested state (e.g. clearing an
            // absent `folder`): no write, no broadcast.
            return Ok(PutAck {
                sha256: existing_sha,
                mtime_ms: mtime_now(),
            });
        };
        let new_bytes = new_content.into_bytes();
        let tmp = abs.with_extension(format!(
            "{}.tmp.{}",
            abs.extension().and_then(|s| s.to_str()).unwrap_or(""),
            std::process::id()
        ));
        std::fs::write(&tmp, &new_bytes).map_err(io_err)?;
        std::fs::rename(&tmp, &abs).map_err(io_err)?;
        let new_sha = sha256_hex(&new_bytes);
        let mtime_ms = mtime_now();
        drop(g);
        self.emit(
            vault_id,
            VaultEvent::Put {
                path: path.to_string(),
                sha256: new_sha.clone(),
                mtime_ms,
                size: new_bytes.len() as u64,
            },
        );
        Ok(PutAck {
            sha256: new_sha,
            mtime_ms,
        })
    }

    fn open_collab(&self, vault_id: &str, path: &str) -> Result<CollabAck, VaultSyncError> {
        let abs = self.file_path(vault_id, path)?;
        // Joining a file's live session is how the collaborative editor
        // writes, so it takes the same permission a save does.
        self.guard_write(vault_id, path)?;
        // A path with no file yet still opens: a document created on a
        // device (offline, `offline.doc.create`) reaches the server this
        // way, and its first write-behind flush creates the file. The
        // ack's sha is then empty — there are no bytes to hash.
        // t[impl offline.doc.create]
        let exists = abs.exists();
        let sha256 = if exists {
            sha256_file(&abs).map_err(io_err)?.0
        } else {
            String::new()
        };
        let doc_id = self.register_collab(vault_id, path)?;
        Ok(CollabAck { doc_id, sha256 })
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in
/// [`Backend::emit`], on every committed write (and on every external
/// edit the watcher forwards).
impl VaultSyncStreamSource for Backend {
    fn changes_hub(&self) -> &architect::PubSub<VaultChange> {
        &self.changes
    }
}

/// A page's YAML frontmatter (the leading `---` … `---` block) as a JSON
/// object string for [`crate::bases::BaseRow::from_parts_full`]. Empty
/// object when there's no frontmatter or it doesn't parse to a map.
fn frontmatter_json(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("---") else {
        return "{}".into();
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return "{}".into();
    };
    let Some(end) = rest.find("\n---") else {
        return "{}".into();
    };
    match serde_yaml::from_str::<serde_json::Value>(&rest[..end]) {
        Ok(v @ serde_json::Value::Object(_)) => v.to_string(),
        _ => "{}".into(),
    }
}

/// A File Root's own bookkeeping at the vault's top level — the marker
/// and the version store. Present once the vault has been adopted as a
/// root (`project.vault.write-path`), and the server's, not the vault's:
/// a client replicating the vault must never receive them, and the
/// manifest must not change because the server started versioning.
fn is_root_internal(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root)
        && path.file_name().is_some_and(|name| {
            name == files_proto::consts::MARKER_FILE || name == files_proto::consts::STORE_DIR
        })
}

/// Walk the files a manifest covers, calling `visit` with each file's
/// absolute path and its root-relative, `/`-separated path. Reads no
/// file: what to do with one is the visitor's.
fn walk_files(
    root: &Path,
    dir: &Path,
    visit: &mut dyn FnMut(&Path, String) -> Result<(), VaultSyncError>,
) -> Result<(), VaultSyncError> {
    for entry in std::fs::read_dir(dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let path = entry.path();
        if is_root_internal(root, &path) {
            continue;
        }
        if path.is_dir() {
            // A node-private directory never leaves this machine — not
            // in the manifest, so no replica or vault client ever asks
            // for what is under it (`vault.private.node-only`).
            // t[impl vault.private.node-only]
            if files_proto::consts::is_private_dir(&path) {
                continue;
            }
            // A nested shelf is somebody else's root. It has its own
            // manifest, and putting its files in this one would make a
            // replica pull them twice and a CRDT open them under two
            // ids — see `crate::shelf_boundary`. This is the manifest
            // half of the same pair `walker::is_skip` implements for
            // the page walk; the two must stay together, exactly as
            // `files::scan::walk_live_tree` and
            // `Registry::conflicting_root` do one layer down.
            if crate::shelf_boundary::is_nested_shelf(root, &path) {
                continue;
            }
            walk_files(root, &path, visit)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| VaultSyncError::Internal("strip prefix".into()))?
            .to_string_lossy()
            .replace('\\', "/");
        visit(&path, rel)?;
    }
    Ok(())
}

/// Every file the manifest covers, hashed — the manifest's rows, for
/// the files `keep` admits (by root-relative path).
///
/// Each file is hashed by streaming it through [`sha256_file`], never
/// read whole: a manifest is asked for on every replica poll and every
/// doc-peer `list`, and a vault holds whatever its people drop into it.
/// Reading each file into a `Vec` to hash it made the peak of one call
/// the size of the largest file, and made every call allocate the whole
/// vault once over.
fn collect(
    root: &Path,
    dir: &Path,
    keep: &dyn Fn(&str) -> bool,
    out: &mut Vec<ManifestEntry>,
) -> Result<(), VaultSyncError> {
    walk_files(root, dir, &mut |path, rel| {
        if !keep(&rel) {
            return Ok(());
        }
        let (sha256, size) = sha256_file(path).map_err(io_err)?;
        let mtime_ms = std::fs::metadata(path)
            .map_err(io_err)?
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
        let now = chrono::Utc::now();
        out.push(ManifestEntry {
            path: rel,
            sha256,
            mtime_ms,
            size,
            created_at: now,
            updated_at: now,
        });
        Ok(())
    })
}

/// After a file left `abs`'s folder: remove the folders it leaves empty,
/// up to but never including `root`. A wiki's tree is its folders, so a
/// move out of the last page of one must not leave a husk behind.
fn prune_empty_parents(root: &Path, abs: &Path) {
    let mut dir = abs.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            break;
        }
        // `remove_dir` only removes an empty directory; anything else
        // (a sibling file, a permission error) ends the walk.
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Write `bytes` to `abs` atomically (temp + rename, so a concurrent read
/// never sees half a body), creating its folder. Returns the new sha and
/// mtime. Call under the write lock.
fn write_file_atomic(abs: &Path, bytes: &[u8]) -> Result<(String, i64), VaultSyncError> {
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(io_err)?;
    }
    let tmp = abs.with_extension(format!(
        "{}.tmp.{}",
        abs.extension().and_then(|s| s.to_str()).unwrap_or(""),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).map_err(io_err)?;
    std::fs::rename(&tmp, abs).map_err(io_err)?;
    let mtime_ms = std::fs::metadata(abs)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Ok((sha256_hex(bytes), mtime_ms))
}

/// The refusal of a write whose `If-Match` failed, carrying the file as
/// it is now.
///
/// The check hashed the file by streaming it (no buffer the size of the
/// file); the bytes are read only here, on the refusal that ships them.
/// The file can change between the two reads (an editor on the host
/// saving it), so `server_sha` is computed from the bytes that are sent
/// rather than reused from the check: a client gets a pair that agrees,
/// and its retry's `If-Match` names what it was actually shown.
fn conflict(abs: &Path) -> VaultSyncError {
    match std::fs::read(abs) {
        Ok(bytes) => VaultSyncError::Conflict {
            server_sha: sha256_hex(&bytes),
            server_bytes: bytes,
        },
        Err(e) => io_err(e),
    }
}

/// Read a scalar frontmatter property as a string. Text and Date
/// round-trip as-is; other kinds (bool/number/list) aren't
/// meaningful for the folder/title/type keys we read.
fn fm_text(fm: &FrontMatter, key: &str) -> Option<String> {
    fm.props
        .iter()
        .find(|p| p.key.eq_ignore_ascii_case(key))
        .and_then(|p| match &p.value {
            PropValue::Text(s) | PropValue::Date(s) => Some(s.clone()),
            _ => None,
        })
}

/// Frontmatter `aliases` / `alias` values, flattened in document
/// order. Accepts the YAML list form (`aliases: [a, b]` / block
/// list) and the legacy comma-separated string form Obsidian
/// still tolerates.
fn fm_aliases(fm: &FrontMatter) -> Vec<String> {
    let mut out = Vec::new();
    for p in &fm.props {
        if !(p.key.eq_ignore_ascii_case("aliases") || p.key.eq_ignore_ascii_case("alias")) {
            continue;
        }
        match &p.value {
            PropValue::List(items) => out.extend(
                items
                    .iter()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty()),
            ),
            PropValue::Text(s) => out.extend(
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            ),
            _ => {}
        }
    }
    out
}

/// Frontmatter `tags` / `tag` values, `#` stripped, in document
/// order. Same forms as [`fm_aliases`] (YAML list or the legacy
/// comma-separated string).
fn fm_tags(fm: &FrontMatter) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let t = raw.trim().trim_start_matches('#');
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_owned());
        }
    };
    for p in &fm.props {
        if !(p.key.eq_ignore_ascii_case("tags") || p.key.eq_ignore_ascii_case("tag")) {
            continue;
        }
        match &p.value {
            PropValue::List(items) => items.iter().for_each(|s| push(s)),
            PropValue::Text(s) => s.split([',', ' ']).for_each(|piece| push(piece)),
            _ => {}
        }
    }
    out
}

/// Whether one page matches every word of a search (lowercased), in its
/// title or its body (properties are not text), and the hit if so —
/// with whether the title alone matched, which ranks first.
fn search_page(raw: &str, path: &str, title: String, words: &[String]) -> Option<(bool, TextHit)> {
    // The body, and the line it starts on (for the excerpt's line).
    let (body, first_line) = match raw
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---"))
    {
        Some((front, body)) => (body, front.lines().count() + 2),
        None => (raw, 1),
    };
    let title_lc = title.to_lowercase();
    let body_lc = body.to_lowercase();
    if !words
        .iter()
        .all(|w| title_lc.contains(w.as_str()) || body_lc.contains(w.as_str()))
    {
        return None;
    }
    let title_hit = words.iter().all(|w| title_lc.contains(w.as_str()));
    let mut excerpt = None;
    let mut matches = 0u32;
    // `skip(1)`: the rest of the closing fence's line.
    let skip = usize::from(first_line > 1);
    for (i, line) in body.lines().enumerate().skip(skip) {
        let lc = line.to_lowercase();
        if words.iter().any(|w| lc.contains(w.as_str())) {
            matches += 1;
            if excerpt.is_none() {
                let text: String = line.trim().chars().take(160).collect();
                excerpt = Some((text, (first_line + i - skip) as u32));
            }
        }
    }
    let (excerpt, line) = excerpt.unwrap_or_else(|| (title.clone(), 0));
    Some((
        title_hit,
        TextHit {
            path: path.to_owned(),
            title,
            excerpt,
            line,
            matches,
        },
    ))
}

/// A page's display title: its `title` property; else, for a record
/// the app named by id (an inbox item's `<uuid>.md`), its first line of
/// text — a name nobody chose reads as noise in a list; else the file
/// name, which is a note's title.
fn page_title(raw: &str, basename: &str, title: Option<String>) -> String {
    if let Some(t) = title {
        return t;
    }
    if Uuid::parse_str(basename).is_ok() {
        let body = raw
            .strip_prefix("---")
            .and_then(|rest| rest.split_once("\n---"))
            .map_or(raw, |(_, body)| body);
        // The fence's closing line comes back as the first (empty) one.
        let first = body.lines().map(str::trim).find(|l| !l.is_empty());
        if let Some(line) = first {
            let line = line.trim_start_matches('#').trim();
            if !line.is_empty() {
                return line.chars().take(80).collect();
            }
        }
    }
    basename.to_string()
}

/// Reduce a `folder` value to a bare parent basename. Handles the
/// Obsidian wikilink form `[[Name|alias]]#heading` as well as a
/// plain string.
fn strip_wikilink(value: &str) -> String {
    let t = value.trim();
    let inner = t
        .strip_prefix("[[")
        .and_then(|x| x.strip_suffix("]]"))
        .unwrap_or(t);
    inner
        .split(['|', '#'])
        .next()
        .unwrap_or(inner)
        .trim()
        .to_string()
}

/// Splice a note's `folder` frontmatter to point at `parent`
/// (`None` clears it). Returns the rewritten content, or `None`
/// when no change is needed. Preserves key order + every other
/// property by editing only the `folder` line's byte range.
fn apply_folder(content: &str, parent: Option<&str>) -> Option<String> {
    let fm = parse_frontmatter(content);
    let existing = fm.as_ref().and_then(|f| {
        f.props
            .iter()
            .find(|p| p.key.eq_ignore_ascii_case("folder"))
    });
    match (parent, existing) {
        // Re-point an existing `folder:` line.
        (Some(p), Some(prop)) => {
            let mut s = String::with_capacity(content.len() + p.len());
            s.push_str(&content[..prop.range.start]);
            s.push_str(&format!("folder: \"[[{p}]]\"\n"));
            s.push_str(&content[prop.range.end..]);
            Some(s)
        }
        // Drop an existing `folder:` line (move to root).
        (None, Some(prop)) => {
            let mut s = String::with_capacity(content.len());
            s.push_str(&content[..prop.range.start]);
            s.push_str(&content[prop.range.end..]);
            Some(s)
        }
        // Insert into an existing frontmatter block, just before
        // the closing `---`.
        (Some(p), None) if fm.is_some() => {
            let at = fm.as_ref().unwrap().closer.start;
            let mut s = String::with_capacity(content.len() + p.len() + 16);
            s.push_str(&content[..at]);
            s.push_str(&format!("folder: \"[[{p}]]\"\n"));
            s.push_str(&content[at..]);
            Some(s)
        }
        // No frontmatter at all — prepend a minimal block.
        (Some(p), None) => Some(format!("---\nfolder: \"[[{p}]]\"\n---\n{content}")),
        // Clearing an absent `folder:` — already at root.
        (None, None) => None,
    }
}

fn io_err(e: std::io::Error) -> VaultSyncError {
    VaultSyncError::Io(e.to_string())
}

/// Read mtime in unix-ms, defaulting to 0 on any error. Matches
/// the `0` fallback used elsewhere in the backend so wire shapes
/// stay consistent.
fn mtime_ms(abs: &Path) -> i64 {
    std::fs::metadata(abs)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// What a path looked like the last time the watcher reported it —
/// the memory that tells a read apart from a write.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    mtime_ms: i64,
    sha256: String,
}

/// Whether an event for `rel` carries new content, remembering it if
/// so.
///
/// notify's inotify mask includes `OPEN`, so merely READING a file
/// reports it as changed, and the debouncer collapses every kind to
/// "any". Forwarded as a `Put`, a read becomes a write downstream: the
/// link sync reads the note to re-sync it, which is an open, which is
/// an event, which is a re-sync — one loop per note, forever. A
/// production flamegraph showed the server burning ~4.8 cores on
/// exactly that. A file whose mtime and hash both match the last
/// report has not changed, whatever inotify says.
fn is_fresh(seen: &mut HashMap<String, Seen>, rel: &str, now: Seen) -> bool {
    if seen.get(rel) == Some(&now) {
        return false;
    }
    seen.insert(rel.to_owned(), now);
    true
}

/// Pump `watcher` events into the in-process broadcast channel and
/// the wire hub — the same pair [`Backend::emit`] writes, so an
/// external edit is indistinguishable from a PUT downstream. Runs
/// on a dedicated OS thread spawned by [`Backend::start_watcher`];
/// exits when the watcher guard drops (closing the sender).
fn forward_watcher_events(
    root: PathBuf,
    rx: std::sync::mpsc::Receiver<watcher::VaultEvent>,
    tx: broadcast::Sender<VaultEvent>,
    hub: architect::PubSub<VaultChange>,
    vault_id: String,
) {
    let mut seen: HashMap<String, Seen> = HashMap::new();
    // The watcher reports canonical paths (macOS's FSEvents: `/private/var`
    // for a root given as `/var`, a symlink), so an event is matched
    // against the root as given and as the filesystem names it — else
    // every external edit under a symlinked root is silently dropped.
    let canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    while let Ok(evt) = rx.recv() {
        let abs = match evt {
            watcher::VaultEvent::Changed { abs_path } => abs_path,
            watcher::VaultEvent::Removed { abs_path } => abs_path,
        };
        let Ok(rel_path) = abs
            .strip_prefix(&root)
            .or_else(|_| abs.strip_prefix(&canonical))
        else {
            continue;
        };
        let rel = rel_path.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            continue;
        }
        // `Changed` events from the debouncer cover both
        // create+modify AND delete. Reload-or-classify by
        // existence — cheaper than disambiguating the raw
        // `notify::EventKind`.
        let payload = if abs.exists() {
            // Hashed by streaming: the event needs the digest and the
            // size, never the bytes.
            match sha256_file(&abs) {
                Ok((sha256, size)) => {
                    let mtime_ms = mtime_ms(&abs);
                    let now = Seen {
                        mtime_ms,
                        sha256: sha256.clone(),
                    };
                    if !is_fresh(&mut seen, &rel, now) {
                        continue;
                    }
                    VaultEvent::Put {
                        path: rel,
                        sha256,
                        mtime_ms,
                        size,
                    }
                }
                Err(e) => {
                    tracing::warn!(?abs, ?e, "watcher: failed to read changed file");
                    continue;
                }
            }
        } else {
            seen.remove(&rel);
            VaultEvent::Delete { path: rel }
        };
        hub.publish(VaultChange {
            vault_id: vault_id.clone(),
            event: payload.clone(),
        });
        if tx.send(payload).is_err() {
            // No active subscribers — keep pumping; new
            // subscribers attach later via the same channel.
            continue;
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_of(&h.finalize())
}

/// A file's SHA-256 (hex) and length, streamed through a fixed buffer.
///
/// The hashing paths here — the manifest, the watcher, an `If-Match`
/// check — need the digest, not the bytes. Streaming makes their cost a
/// 16 KiB stack buffer whatever the file is, where `std::fs::read`
/// allocated the whole file (and, for a vault holding a bounce or a
/// PDF, made that the peak of the call).
fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    let mut len = 0u64;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..n]);
        len += n as u64;
    }
    Ok((hex_of(&h.finalize()), len))
}

fn hex_of(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    /// A page matches when every word is in its title or its body — never
    /// its properties — and the hit carries the first matching line.
    #[test]
    fn a_search_reads_the_body_and_the_title_but_not_the_properties() {
        let raw =
            "---\nstatus: mixing\n---\n\nTrack list\nThe opener is loud\nThe closer is quiet\n";
        let words = |q: &str| {
            q.split_whitespace()
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        };
        let hit = |q: &str| super::search_page(raw, "Set.md", "Setlist".into(), &words(q));

        let (title_hit, h) = hit("the LOUD").expect("both words are in the body");
        assert!(!title_hit);
        assert_eq!(
            (h.excerpt.as_str(), h.line, h.matches),
            ("The opener is loud", 6, 2)
        );
        assert!(
            hit("setlist").expect("the title").0,
            "a title match ranks first"
        );
        assert!(
            hit("setlist quiet").is_some(),
            "words may split across title and body"
        );
        assert!(hit("mixing").is_none(), "a property is not text");
        assert!(hit("opener absent").is_none(), "every word must match");
    }

    /// A record named by id lists by what it says; a note by its name.
    #[test]
    fn a_record_named_by_id_is_titled_by_its_first_line() {
        let id = "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a01";
        let inbox = "---\ntype: inbox-item\n---\n\nCall the piano tuner about Thursday\n";
        assert_eq!(
            super::page_title(inbox, id, None),
            "Call the piano tuner about Thursday"
        );
        assert_eq!(super::page_title("# Heading\nmore", id, None), "Heading");
        assert_eq!(super::page_title("---\na: 1\n---\n", id, None), id);
        assert_eq!(
            super::page_title(inbox, id, Some("Chosen".into())),
            "Chosen"
        );
        assert_eq!(
            super::page_title("Setlist ideas", "Untitled 1", None),
            "Untitled 1"
        );
    }

    /// A read is not a write: the same mtime and hash reported twice
    /// is one event, not two; a different hash (an edit) or a different
    /// mtime (a touch) is fresh; a delete forgets the path so a recreate
    /// is fresh again.
    #[test]
    fn a_reread_file_is_not_a_change() {
        let mut seen = super::HashMap::new();
        let same = || super::Seen {
            mtime_ms: 1_700_000_000_000,
            sha256: "abc".into(),
        };
        assert!(
            super::is_fresh(&mut seen, "Note.md", same()),
            "first sighting"
        );
        assert!(
            !super::is_fresh(&mut seen, "Note.md", same()),
            "an open, not an edit"
        );
        assert!(
            super::is_fresh(
                &mut seen,
                "Note.md",
                super::Seen {
                    mtime_ms: 1_700_000_000_000,
                    sha256: "def".into()
                }
            ),
            "new content"
        );
        assert!(
            super::is_fresh(
                &mut seen,
                "Note.md",
                super::Seen {
                    mtime_ms: 1_700_000_001_000,
                    sha256: "def".into()
                }
            ),
            "a touch moves the mtime and is reported"
        );
        seen.remove("Note.md");
        assert!(
            super::is_fresh(&mut seen, "Note.md", same()),
            "recreated after delete"
        );
    }

    use super::*;

    fn make_backend() -> (tempfile::TempDir, Backend) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Backend::single("v1", tmp.path().to_path_buf()).unwrap();
        (tmp, backend)
    }

    #[test]
    fn a_streamed_hash_is_the_hash_of_the_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        // Bigger than the stream buffer and not a multiple of it, so the
        // last partial read is in the digest.
        let bytes: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
        let path = tmp.path().join("f.bin");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            (sha256_hex(&bytes), bytes.len() as u64)
        );
        std::fs::write(&path, b"").unwrap();
        assert_eq!(sha256_file(&path).unwrap(), (sha256_hex(b""), 0));
    }

    #[test]
    fn paths_and_a_filtered_manifest_agree_with_the_manifest() {
        let (tmp, b) = make_backend();
        std::fs::create_dir_all(tmp.path().join("Notes")).unwrap();
        std::fs::write(tmp.path().join("Notes/a.md"), b"# a").unwrap();
        std::fs::write(tmp.path().join("b.md"), b"# b").unwrap();
        std::fs::write(tmp.path().join("bounce.wav"), vec![1u8; 70_000]).unwrap();

        let full = b.manifest("v1").unwrap();
        let mut from_manifest: Vec<String> = full.files.iter().map(|f| f.path.clone()).collect();
        from_manifest.sort();
        let mut paths = b.paths("v1").unwrap();
        paths.sort();
        assert_eq!(
            paths, from_manifest,
            "a path listing is the manifest's paths"
        );

        let md = b.manifest_where("v1", |p| p.ends_with(".md")).unwrap();
        let mut md_paths: Vec<&str> = md.files.iter().map(|f| f.path.as_str()).collect();
        md_paths.sort_unstable();
        assert_eq!(md_paths, ["Notes/a.md", "b.md"]);
        for row in &md.files {
            let same = full.files.iter().find(|f| f.path == row.path).unwrap();
            assert_eq!((&row.sha256, row.size), (&same.sha256, same.size));
        }
        let wav = full.files.iter().find(|f| f.path == "bounce.wav").unwrap();
        assert_eq!(wav.size, 70_000, "a streamed row still carries the size");
    }

    #[test]
    fn manifest_empty_vault() {
        let (_tmp, b) = make_backend();
        let m = b.manifest("v1").unwrap();
        assert_eq!(m.vault_id, "v1");
        assert!(m.files.is_empty());
    }

    /// A node-private directory is out of the manifest, and its files
    /// cannot be read or written by naming them either.
    // t[verify vault.private.node-only]
    #[test]
    fn private_directories_never_leave_the_node() {
        let (tmp, b) = make_backend();
        let private = tmp.path().join("Records/contacts/.sync");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::write(private.join(crate::private::PRIVATE_MARKER), b"").unwrap();
        std::fs::write(private.join("acct.md"), b"secret-ish").unwrap();
        std::fs::write(tmp.path().join("Records/contacts/ada.md"), b"Ada").unwrap();

        let paths: Vec<String> = b
            .manifest("v1")
            .unwrap()
            .files
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(paths, vec!["Records/contacts/ada.md".to_string()]);
        assert!(matches!(
            b.get_file("v1", "Records/contacts/.sync/acct.md"),
            Err(VaultSyncError::NotFound)
        ));
        assert!(
            b.put_file(
                "v1",
                "Records/contacts/.sync/acct.md",
                b"overwrite".to_vec(),
                IfMatch::Force
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(private.join("acct.md")).unwrap(),
            b"secret-ish"
        );
    }

    #[test]
    fn unknown_vault_id_returns_not_found() {
        let (_tmp, b) = make_backend();
        assert!(matches!(b.manifest("nope"), Err(VaultSyncError::NotFound)));
    }

    #[test]
    fn put_then_get_round_trips() {
        let (_tmp, b) = make_backend();
        b.put_file("v1", "hello.md", b"body".to_vec(), IfMatch::CreateOnly)
            .unwrap();
        let got = b.get_file("v1", "hello.md").unwrap();
        assert_eq!(got.0, b"body");
    }

    #[test]
    fn if_match_create_only_refuses_existing_file() {
        let (_tmp, b) = make_backend();
        b.put_file("v1", "x.md", b"first".to_vec(), IfMatch::CreateOnly)
            .unwrap();
        let err = b
            .put_file("v1", "x.md", b"second".to_vec(), IfMatch::CreateOnly)
            .unwrap_err();
        assert!(matches!(err, VaultSyncError::Conflict { .. }));
    }

    #[test]
    fn if_match_sha_mismatch_returns_conflict() {
        let (_tmp, b) = make_backend();
        b.put_file("v1", "x.md", b"v1".to_vec(), IfMatch::CreateOnly)
            .unwrap();
        let err = b
            .put_file(
                "v1",
                "x.md",
                b"v2".to_vec(),
                IfMatch::Sha("deadbeef".into()),
            )
            .unwrap_err();
        match err {
            VaultSyncError::Conflict {
                server_sha,
                server_bytes,
            } => {
                assert_eq!(server_bytes, b"v1");
                assert_eq!(server_sha, sha256_hex(b"v1"));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    /// A refused delete ships the file as it is, and the sha it names is
    /// the sha of exactly those bytes, so the client's retry can match
    /// what it was shown.
    #[test]
    fn if_match_delete_conflict_names_the_sha_of_the_bytes_it_ships() {
        let (_tmp, b) = make_backend();
        b.put_file("v1", "x.md", b"kept".to_vec(), IfMatch::CreateOnly)
            .unwrap();
        let err = b
            .delete_file("v1", "x.md", IfMatch::Sha("deadbeef".into()))
            .unwrap_err();
        match err {
            VaultSyncError::Conflict {
                server_sha,
                server_bytes,
            } => {
                assert_eq!(server_bytes, b"kept");
                assert_eq!(server_sha, sha256_hex(&server_bytes));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert_eq!(b.get_file("v1", "x.md").unwrap().0, b"kept");
    }

    #[test]
    fn rejects_traversal_attempts() {
        let (_tmp, b) = make_backend();
        let err = b
            .put_file("v1", "../escape.md", b"x".to_vec(), IfMatch::Force)
            .unwrap_err();
        assert!(matches!(err, VaultSyncError::BadPath));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn put_broadcasts_event() {
        let (_tmp, b) = make_backend();
        let tx = b.channel("v1").await;
        let mut rx = tx.subscribe();
        let backend = b.clone();
        tokio::task::spawn_blocking(move || {
            backend
                .put_file("v1", "hi.md", b"x".to_vec(), IfMatch::CreateOnly)
                .unwrap();
        })
        .await
        .unwrap();
        let evt = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv ok");
        match evt {
            VaultEvent::Put { path, size, .. } => {
                assert_eq!(path, "hi.md");
                assert_eq!(size, 1);
            }
            other => panic!("expected Put, got {other:?}"),
        }
    }

    #[test]
    fn under_parent_auto_creates_vault_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let b = Backend::under_parent(tmp.path().to_path_buf()).unwrap();
        // Any vault_id works; the subdir materializes on first
        // write.
        b.put_file("fresh-vault", "n.md", b"x".to_vec(), IfMatch::CreateOnly)
            .unwrap();
        assert!(tmp.path().join("fresh-vault").join("n.md").exists());
        // Manifest of an unwritten id is empty, not an error.
        assert!(b.manifest("never-touched").unwrap().files.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watcher_forwards_external_writes() {
        let (tmp, b) = make_backend();
        let tx = b.channel("v1").await;
        let mut rx = tx.subscribe();
        // Hold the watcher alive across the write+recv.
        let _watch = b.start_watcher("v1").await.expect("start watcher");
        // notify-debouncer-mini ignores events that fire before
        // the watcher has fully attached on some platforms; a
        // brief settle prevents flakes.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Write a file directly to disk — bypasses the backend
        // entirely, simulating an external editor.
        std::fs::write(tmp.path().join("ext.md"), b"hi from vim").unwrap();

        // The debounce window is 500ms; wait up to 3s for the
        // event to land.
        let evt = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .expect("watcher event timeout")
            .expect("rx recv ok");
        match evt {
            VaultEvent::Put { path, size, .. } => {
                assert_eq!(path, "ext.md");
                assert_eq!(size, 11);
            }
            other => panic!("expected Put, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_vault_isolation() {
        let tmp_a = tempfile::tempdir().unwrap();
        let tmp_b = tempfile::tempdir().unwrap();
        let mut roots = HashMap::new();
        roots.insert("a".into(), tmp_a.path().to_path_buf());
        roots.insert("b".into(), tmp_b.path().to_path_buf());
        let backend = Backend::with_roots(roots);
        let b1 = backend.clone();
        tokio::task::spawn_blocking(move || {
            b1.put_file("a", "note.md", b"hello-a".to_vec(), IfMatch::CreateOnly)
                .unwrap();
        })
        .await
        .unwrap();
        assert_eq!(backend.manifest("b").unwrap().files.len(), 0);
        assert_eq!(backend.manifest("a").unwrap().files.len(), 1);
    }

    #[test]
    fn a_move_never_overwrites_and_a_rename_carries_its_links() {
        let (_tmp, b) = make_backend();
        b.put_file(
            "v1",
            "Concepts/Ionian.md",
            b"# Ionian\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();
        b.put_file(
            "v1",
            "Modes.md",
            b"The first is [[Ionian|the major mode]].\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();
        b.put_file("v1", "Taken.md", b"x".to_vec(), IfMatch::CreateOnly)
            .unwrap();

        assert!(matches!(
            b.move_file("v1", "Concepts/Ionian.md", "Taken.md", IfMatch::Force),
            Err(VaultSyncError::Conflict { .. })
        ));

        // A move to another folder keeps the name: no link changes.
        let moved = b
            .move_file(
                "v1",
                "Concepts/Ionian.md",
                "Scales/Ionian.md",
                IfMatch::Force,
            )
            .unwrap();
        assert!(moved.relinked.is_empty());
        assert!(matches!(
            b.get_file("v1", "Concepts/Ionian.md"),
            Err(VaultSyncError::NotFound)
        ));
        assert!(
            !b.root("v1").unwrap().join("Concepts").exists(),
            "the emptied folder was left behind"
        );

        // A rename rewrites the link, alias intact.
        let renamed = b
            .move_file(
                "v1",
                "Scales/Ionian.md",
                "Scales/Major Scale.md",
                IfMatch::Sha(moved.sha256),
            )
            .unwrap();
        assert_eq!(renamed.relinked, vec!["Modes.md".to_string()]);
        assert_eq!(
            b.get_file("v1", "Modes.md").unwrap().0,
            b"The first is [[Major Scale|the major mode]].\n"
        );
        assert_eq!(
            b.get_file("v1", "Scales/Major Scale.md").unwrap().0,
            b"# Ionian\n"
        );
    }

    #[test]
    fn open_collab_registers_and_reverse_resolves() {
        let (_tmp, b) = make_backend();
        b.put_file("v1", "n.md", b"body".to_vec(), IfMatch::CreateOnly)
            .unwrap();
        let ack = b.open_collab("v1", "n.md").unwrap();
        assert_eq!(ack.doc_id, collab_doc_id("v1", "n.md"));
        assert_eq!(ack.sha256, sha256_hex(b"body"));
        assert_eq!(
            b.collab_route(ack.doc_id),
            Some(("v1".to_string(), "n.md".to_string()))
        );
        // Idempotent: same id, refreshed sha.
        b.put_file("v1", "n.md", b"body2".to_vec(), IfMatch::Force)
            .unwrap();
        let again = b.open_collab("v1", "n.md").unwrap();
        assert_eq!(again.doc_id, ack.doc_id);
        assert_eq!(again.sha256, sha256_hex(b"body2"));
        // Unregistered ids resolve to nothing — the admission gate.
        assert_eq!(b.collab_route(collab_doc_id("v1", "other.md")), None);
        // A path with no file yet registers too (a document created
        // offline arrives this way); there is no sha to report.
        // t[verify offline.doc.create]
        let fresh = b.open_collab("v1", "missing.md").unwrap();
        assert_eq!(fresh.sha256, "");
        assert_eq!(
            b.collab_route(fresh.doc_id),
            Some(("v1".to_string(), "missing.md".to_string()))
        );
        // A path outside the vault still refuses.
        assert!(b.open_collab("v1", "../escape.md").is_err());
    }

    fn meta<'a>(idx: &'a vault_proto::FolderIndex, base: &str) -> &'a PageMeta {
        idx.pages
            .iter()
            .find(|p| p.basename == base)
            .unwrap_or_else(|| panic!("no page `{base}` in index"))
    }

    #[test]
    fn folder_index_parses_frontmatter_and_resolves_parent() {
        let (_tmp, b) = make_backend();
        // Root folder note (no `folder`), a child note pointing at
        // it via a wikilink, and a plain note with no frontmatter.
        b.put_file(
            "v1",
            "Wisdom/Wisdom.md",
            b"---\ntitle: Wisdom\ntype: folder\n---\n# Wisdom\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();
        b.put_file(
            "v1",
            "Wisdom/Plans.md",
            b"---\ntitle: Plans rot\nfolder: \"[[Wisdom]]\"\n---\nbody\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();
        b.put_file(
            "v1",
            "loose.md",
            b"no frontmatter here\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();

        let idx = b.folder_index("v1").unwrap();
        assert_eq!(idx.pages.len(), 3);

        let root = meta(&idx, "Wisdom");
        assert_eq!(root.title, "Wisdom");
        assert_eq!(root.page_type, "folder");
        assert_eq!(root.folder, "", "root folder note has no parent");

        let child = meta(&idx, "Plans");
        assert_eq!(child.title, "Plans rot");
        assert_eq!(
            child.folder, "Wisdom",
            "wikilink resolved to parent basename"
        );

        let loose = meta(&idx, "loose");
        assert_eq!(loose.title, "loose", "title falls back to basename");
        assert_eq!(loose.folder, "");
    }

    #[test]
    fn set_folder_inserts_replaces_and_clears_preserving_order() {
        let (_tmp, b) = make_backend();
        b.put_file(
            "v1",
            "n.md",
            b"---\ntitle: N\ntags: [a]\n---\nbody\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();

        // Insert into an existing block (before the closer), other
        // keys + order preserved.
        b.set_folder("v1", "n.md", Some("Home".into()), IfMatch::Force)
            .unwrap();
        let after_insert = String::from_utf8(b.get_file("v1", "n.md").unwrap().0).unwrap();
        assert_eq!(
            after_insert,
            "---\ntitle: N\ntags: [a]\nfolder: \"[[Home]]\"\n---\nbody\n"
        );

        // Re-point the existing folder line.
        b.set_folder("v1", "n.md", Some("Work".into()), IfMatch::Force)
            .unwrap();
        let after_repoint = String::from_utf8(b.get_file("v1", "n.md").unwrap().0).unwrap();
        assert!(after_repoint.contains("folder: \"[[Work]]\""));
        assert!(!after_repoint.contains("Home"));

        // Clear it (move to root) — line removed, rest intact.
        b.set_folder("v1", "n.md", None, IfMatch::Force).unwrap();
        let after_clear = String::from_utf8(b.get_file("v1", "n.md").unwrap().0).unwrap();
        assert_eq!(after_clear, "---\ntitle: N\ntags: [a]\n---\nbody\n");
    }

    #[test]
    fn set_folder_creates_block_when_no_frontmatter() {
        let (_tmp, b) = make_backend();
        b.put_file(
            "v1",
            "bare.md",
            b"just text\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();
        b.set_folder("v1", "bare.md", Some("Inbox".into()), IfMatch::Force)
            .unwrap();
        let out = String::from_utf8(b.get_file("v1", "bare.md").unwrap().0).unwrap();
        assert_eq!(out, "---\nfolder: \"[[Inbox]]\"\n---\njust text\n");
    }

    /// A `.base` filtering `type: recipe` should list the cookbook —
    /// which lives under the wiki root, not the vault root. Before
    /// `with_recipe_roots` these rows didn't exist and the view came
    /// back empty.
    #[test]
    fn base_views_include_recipes_from_the_wiki_root() {
        let vault_dir = tempfile::tempdir().unwrap();
        let wiki_dir = tempfile::tempdir().unwrap();

        std::fs::write(
            vault_dir.path().join("Cookbook.base"),
            "filters:\n  and:\n    - 'type == \"recipe\"'\n\
             views:\n  - name: All recipes\n    type: table\n    order: [title, servings]\n",
        )
        .unwrap();
        // A vault note that must NOT show up — proves the filter runs
        // rather than everything being swept in.
        std::fs::write(
            vault_dir.path().join("note.md"),
            "---\ntype: meal\ntitle: Tuesday\n---\nbody\n",
        )
        .unwrap();

        std::fs::create_dir_all(wiki_dir.path().join("Knowledge/Cookbook")).unwrap();
        std::fs::write(
            wiki_dir.path().join("Knowledge/Cookbook/oatmeal.cook"),
            ">> title: Oatmeal\n>> servings: 1\n\nStir @oats{50%g}.\n",
        )
        .unwrap();

        let mut roots = HashMap::new();
        roots.insert("v1".to_string(), vault_dir.path().to_path_buf());
        let mut recipe_roots = HashMap::new();
        recipe_roots.insert("v1".to_string(), wiki_dir.path().to_path_buf());
        let backend = Backend::with_roots(roots).with_recipe_roots(recipe_roots);

        let views = backend.base_views("v1", "Cookbook.base").unwrap();
        assert_eq!(views.len(), 1, "one view declared");
        let rows: Vec<_> = views[0].groups.iter().flat_map(|g| &g.rows).collect();
        assert_eq!(
            rows.len(),
            1,
            "the meal note must not match a recipe filter"
        );
        assert_eq!(rows[0].title, "Oatmeal");
        assert_eq!(
            rows[0].path, "Knowledge/Cookbook/oatmeal.cook",
            "path stays the recipe's own, so a click can open cook mode"
        );
        assert_eq!(rows[0].cells, vec!["Oatmeal".to_string(), "1".to_string()]);
    }

    /// The default backend has no recipe roots, so nothing changes for
    /// vaults that never register one.
    #[test]
    fn base_views_without_recipe_roots_see_only_vault_pages() {
        let (_tmp, b) = make_backend();
        b.put_file(
            "v1",
            "Recipes.base",
            b"filters:\n  and:\n    - 'type == \"recipe\"'\nviews:\n  - name: All\n    type: table\n    order: [title]\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .unwrap();
        let views = b.base_views("v1", "Recipes.base").unwrap();
        let rows: usize = views[0].groups.iter().map(|g| g.rows.len()).sum();
        assert_eq!(rows, 0);
    }
}
