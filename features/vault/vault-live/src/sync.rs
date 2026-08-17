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
    ManifestEntry, PageMeta, PutAck, VaultChange, VaultEvent, VaultSync, VaultSyncError,
    VaultSyncStreamSource, collab_doc_id,
};

use crate::vault::Vault;
use crate::watcher::{self, WatchError};
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
    Explicit(HashMap<String, PathBuf>),
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
        Self::from_layout(Layout::Explicit(roots))
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

    /// Resolve `vault_id` to an absolute root path. Returns
    /// [`VaultSyncError::NotFound`] only in `Explicit` mode
    /// when the id is unregistered; `UnderParent` always
    /// succeeds.
    fn root(&self, vault_id: &str) -> Result<PathBuf, VaultSyncError> {
        match &self.layout {
            Layout::Explicit(map) => map.get(vault_id).cloned().ok_or(VaultSyncError::NotFound),
            Layout::UnderParent(parent) => Ok(parent.join(vault_id)),
        }
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
        let (rx, guard) = watcher::watch(root.clone(), WATCHER_DEBOUNCE)?;

        let thread_name = format!("vault-sync-watcher:{vault_id}");
        let hub = self.changes.clone();
        let id = vault_id.to_string();
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || forward_watcher_events(root, rx, tx, hub, id))
            .map_err(|e| WatchError::Notify(format!("spawn watcher thread: {e}")))?;

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
        collect(&dir, &dir, &mut entries)?;
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
        let g = self
            .write_lock
            .lock()
            .expect("vault::sync write_lock poisoned");
        let existing_sha = if abs.exists() {
            let bytes = std::fs::read(&abs).map_err(io_err)?;
            Some(sha256_hex(&bytes))
        } else {
            None
        };
        match (&if_match, existing_sha.as_deref()) {
            (IfMatch::CreateOnly, Some(_)) => return Err(conflict(&abs, existing_sha.as_deref())),
            (IfMatch::Sha(want), Some(have)) if want != have => {
                return Err(conflict(&abs, existing_sha.as_deref()));
            }
            _ => {}
        }
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        // Atomic write via temp+rename so concurrent reads never
        // see a half-written body.
        let tmp = abs.with_extension(format!(
            "{}.tmp.{}",
            abs.extension().and_then(|s| s.to_str()).unwrap_or(""),
            std::process::id()
        ));
        std::fs::write(&tmp, &bytes).map_err(io_err)?;
        std::fs::rename(&tmp, &abs).map_err(io_err)?;
        let new_sha = sha256_hex(&bytes);
        let mtime_ms = std::fs::metadata(&abs)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
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
        let g = self
            .write_lock
            .lock()
            .expect("vault::sync write_lock poisoned");
        if !abs.exists() {
            // Idempotent: missing path = success, no broadcast.
            return Ok(());
        }
        if let IfMatch::Sha(want) = &if_match {
            let bytes = std::fs::read(&abs).map_err(io_err)?;
            let have = sha256_hex(&bytes);
            if *want != have {
                return Err(VaultSyncError::Conflict {
                    server_sha: have,
                    server_bytes: bytes,
                });
            }
        }
        std::fs::remove_file(&abs).map_err(io_err)?;
        drop(g);
        self.emit(
            vault_id,
            VaultEvent::Delete {
                path: path.to_string(),
            },
        );
        Ok(())
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
                    title: get("title").unwrap_or_else(|| p.basename.clone()),
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

    fn set_folder(
        &self,
        vault_id: &str,
        path: &str,
        parent: Option<String>,
        if_match: IfMatch,
    ) -> Result<PutAck, VaultSyncError> {
        let abs = self.file_path(vault_id, path)?;
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
        if !abs.exists() {
            return Err(VaultSyncError::NotFound);
        }
        let bytes = std::fs::read(&abs).map_err(io_err)?;
        let doc_id = collab_doc_id(vault_id, path);
        self.collab
            .write()
            .expect("vault::sync collab map poisoned")
            .insert(doc_id, (vault_id.to_string(), path.to_string()));
        Ok(CollabAck {
            doc_id,
            sha256: sha256_hex(&bytes),
        })
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

fn collect(root: &Path, dir: &Path, out: &mut Vec<ManifestEntry>) -> Result<(), VaultSyncError> {
    for entry in std::fs::read_dir(dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| VaultSyncError::Internal("strip prefix".into()))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path).map_err(io_err)?;
        let sha256 = sha256_hex(&bytes);
        let mtime_ms = std::fs::metadata(&path)
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
            size: bytes.len() as u64,
            created_at: now,
            updated_at: now,
        });
    }
    Ok(())
}

fn conflict(abs: &Path, existing_sha: Option<&str>) -> VaultSyncError {
    let bytes = std::fs::read(abs).unwrap_or_default();
    VaultSyncError::Conflict {
        server_sha: existing_sha.unwrap_or("").to_string(),
        server_bytes: bytes,
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
    while let Ok(evt) = rx.recv() {
        let abs = match evt {
            watcher::VaultEvent::Changed { abs_path } => abs_path,
            watcher::VaultEvent::Removed { abs_path } => abs_path,
        };
        let Ok(rel_path) = abs.strip_prefix(&root) else {
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
            match std::fs::read(&abs) {
                Ok(bytes) => VaultEvent::Put {
                    path: rel,
                    sha256: sha256_hex(&bytes),
                    mtime_ms: mtime_ms(&abs),
                    size: bytes.len() as u64,
                },
                Err(e) => {
                    tracing::warn!(?abs, ?e, "watcher: failed to read changed file");
                    continue;
                }
            }
        } else {
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
    let out = h.finalize();
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend() -> (tempfile::TempDir, Backend) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Backend::single("v1", tmp.path().to_path_buf()).unwrap();
        (tmp, backend)
    }

    #[test]
    fn manifest_empty_vault() {
        let (_tmp, b) = make_backend();
        let m = b.manifest("v1").unwrap();
        assert_eq!(m.vault_id, "v1");
        assert!(m.files.is_empty());
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
            VaultSyncError::Conflict { server_bytes, .. } => assert_eq!(server_bytes, b"v1"),
            other => panic!("expected Conflict, got {other:?}"),
        }
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
        // Missing path refuses registration.
        assert!(matches!(
            b.open_collab("v1", "missing.md"),
            Err(VaultSyncError::NotFound)
        ));
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
