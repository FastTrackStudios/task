//! [`SyncDaemon`] — the reconcile engine as a long-lived agent (issue
//! #265). It owns the device identity, the set of roots the user chose
//! to sync (with their slices), a **live status store** that the
//! reconcile progress observer updates as chunks land, and a global /
//! per-root pause flag. [`SyncDaemon::tick`] reconciles every chosen,
//! unpaused root against a peer that serves `files_sync::SyncService`.
//!
//! The daemon holds a local backend (its replica store) and, per root,
//! a client onto the coordinator's sync surface. It is deliberately a
//! *pull* engine (see the `files_sync` module doc): "keep syncing"
//! means ticking `reconcile` on a schedule, which is re-runnable and
//! resumable, so a tick interrupted by a crash simply resumes next tick.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use files::{FilesBackend, FilesService as _};
use files_sync::{SyncObserver, SyncServiceClient, reconcile_with_progress};
use uuid::Uuid;

use crate::error::{DaemonError, Result};
use crate::identity::DeviceIdentity;
use crate::model::{DaemonStatus, FileProgress, RootStatus, RootSyncState};

/// What the daemon knows about one root it is set to sync.
struct SyncedRoot {
    name: String,
    slice: Vec<String>,
    paused: bool,
    /// The peer that serves this root's content (the coordinator, or
    /// another replica). One client per root keeps the wiring simple.
    peer: SyncServiceClient,
}

/// The mutable per-root status the observer writes and `status` reads.
#[derive(Default)]
struct RootRuntimeStatus {
    state: RootSyncState,
    files: Vec<FileProgress>,
    chunks_fetched: u64,
    chunks_skipped: u64,
    last_synced_at: Option<chrono::DateTime<Utc>>,
    last_error: Option<String>,
}

/// The shared status map the [`SyncObserver`] mutates during a pull and
/// [`SyncDaemon::status`] snapshots. Behind one `Mutex` so a progress
/// update is a cheap lock + field write, never blocking the pull.
#[derive(Clone)]
struct LiveStatus {
    roots: Arc<Mutex<BTreeMap<Uuid, RootRuntimeStatus>>>,
}

impl SyncObserver for LiveStatus {
    fn scan_started(&self, root_id: Uuid) {
        let mut map = self.roots.lock().expect("status lock");
        let s = map.entry(root_id).or_default();
        s.state = RootSyncState::Syncing;
        s.files.clear();
        s.last_error = None;
    }

    fn file_started(
        &self,
        root_id: Uuid,
        path: &str,
        total_chunks: usize,
        resident_chunks: usize,
        logical_bytes: u64,
    ) {
        let mut map = self.roots.lock().expect("status lock");
        let s = map.entry(root_id).or_default();
        // Most-recently-touched first; a re-touch moves to the front.
        s.files.retain(|f| f.path != path);
        s.files.insert(
            0,
            FileProgress {
                path: path.to_string(),
                chunks_done: resident_chunks as u32,
                chunks_total: total_chunks as u32,
                bytes_done: 0,
                logical_bytes,
                done: false,
            },
        );
    }

    fn file_progress(&self, root_id: Uuid, path: &str, chunks_done: usize, bytes_done: u64) {
        let mut map = self.roots.lock().expect("status lock");
        let s = map.entry(root_id).or_default();
        if let Some(f) = s.files.iter_mut().find(|f| f.path == path) {
            f.chunks_done = chunks_done as u32;
            f.bytes_done = bytes_done;
        }
    }

    fn file_finished(&self, root_id: Uuid, path: &str) {
        let mut map = self.roots.lock().expect("status lock");
        let s = map.entry(root_id).or_default();
        if let Some(f) = s.files.iter_mut().find(|f| f.path == path) {
            f.done = true;
            f.chunks_done = f.chunks_total;
            f.bytes_done = f.logical_bytes;
        }
    }

    fn pull_finished(&self, root_id: Uuid, error: Option<&str>) {
        let mut map = self.roots.lock().expect("status lock");
        let s = map.entry(root_id).or_default();
        match error {
            None => {
                s.state = RootSyncState::Idle;
                s.last_synced_at = Some(Utc::now());
                s.files.clear();
            }
            Some(e) => {
                s.state = RootSyncState::Error;
                s.last_error = Some(e.to_string());
            }
        }
    }
}

/// The sync daemon.
#[derive(Clone)]
pub struct SyncDaemon {
    inner: Arc<DaemonInner>,
}

struct DaemonInner {
    backend: FilesBackend,
    identity: Mutex<DeviceIdentity>,
    data_dir: std::path::PathBuf,
    roots: Mutex<BTreeMap<Uuid, SyncedRoot>>,
    /// The coordinator this daemon pulls from — its `SyncService` peer.
    /// Set once the daemon knows where to sync from; the control
    /// surface's `set_sync_choice` uses it.
    coordinator: Mutex<Option<SyncServiceClient>>,
    live: LiveStatus,
    paused: std::sync::atomic::AtomicBool,
    events: EventHub,
}

impl SyncDaemon {
    /// Open a daemon over `backend` (its local replica store),
    /// loading — or minting — this machine's device identity from
    /// `data_dir`.
    pub fn open(backend: FilesBackend, data_dir: impl Into<std::path::PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        let identity = DeviceIdentity::load_or_create(&data_dir)?;
        Ok(Self {
            inner: Arc::new(DaemonInner {
                backend,
                identity: Mutex::new(identity),
                data_dir,
                roots: Mutex::new(BTreeMap::new()),
                coordinator: Mutex::new(None),
                live: LiveStatus {
                    roots: Arc::new(Mutex::new(BTreeMap::new())),
                },
                paused: std::sync::atomic::AtomicBool::new(false),
                events: EventHub::default(),
            }),
        })
    }

    /// This machine's device id.
    #[must_use]
    pub fn device_id(&self) -> Uuid {
        self.inner.identity.lock().expect("identity lock").device_id
    }

    /// Record the enrollment secret the coordinator minted (persisted).
    pub fn record_enrollment(&self, secret: String) -> Result<()> {
        self.inner
            .identity
            .lock()
            .expect("identity lock")
            .record_secret(&self.inner.data_dir, secret)
    }

    /// Point the daemon at the coordinator it pulls from.
    pub fn set_coordinator(&self, peer: SyncServiceClient) {
        *self.inner.coordinator.lock().expect("coordinator lock") = Some(peer);
    }

    /// Choose `root_id` for sync against the coordinator, resolving the
    /// root's name from its local record — the control surface's entry
    /// point (the id is all the app has).
    pub async fn choose_root(&self, root_id: Uuid, slice: Vec<String>) -> Result<DaemonStatus> {
        let peer = self
            .inner
            .coordinator
            .lock()
            .expect("coordinator lock")
            .clone()
            .ok_or_else(|| DaemonError::BadRequest("daemon has no coordinator set".into()))?;
        let name = self.inner.backend.get_root(root_id).await?.name;
        self.set_sync_choice(root_id, &name, slice, peer).await?;
        Ok(self.status())
    }

    /// Choose `root_id` for sync against `peer`, with an optional
    /// selective-sync slice. The slice is stored as the root's
    /// hydration policy, so materialize keeps matching paths resident
    /// and the rest as stubs (issue #263) — that is what makes it a
    /// partial replica.
    pub async fn set_sync_choice(
        &self,
        root_id: Uuid,
        name: &str,
        slice: Vec<String>,
        peer: SyncServiceClient,
    ) -> Result<()> {
        // Always write the policy — including an empty one — so
        // re-choosing a root with slice=[] ("the whole root") CLEARS a
        // stale partial policy instead of leaving the replica silently
        // partial (PR #292 review). An empty policy means materialize
        // hydrates everything.
        self.inner
            .backend
            .set_hydration_policy(root_id, slice.clone())
            .await?;
        let mut roots = self.inner.roots.lock().expect("roots lock");
        roots.insert(
            root_id,
            SyncedRoot {
                name: name.to_string(),
                slice,
                paused: false,
                peer,
            },
        );
        drop(roots);
        self.inner.events.publish(self.status());
        Ok(())
    }

    /// Stop syncing `root_id`. Local content is untouched.
    pub fn remove_sync_choice(&self, root_id: Uuid) {
        self.inner
            .roots
            .lock()
            .expect("roots lock")
            .remove(&root_id);
        self.inner
            .live
            .roots
            .lock()
            .expect("status lock")
            .remove(&root_id);
        self.inner.events.publish(self.status());
    }

    /// Pause all syncing (or one root).
    pub fn pause(&self, root_id: Option<Uuid>) {
        match root_id {
            None => self
                .inner
                .paused
                .store(true, std::sync::atomic::Ordering::SeqCst),
            Some(id) => {
                if let Some(r) = self.inner.roots.lock().expect("roots lock").get_mut(&id) {
                    r.paused = true;
                }
                if let Some(s) = self
                    .inner
                    .live
                    .roots
                    .lock()
                    .expect("status lock")
                    .get_mut(&id)
                {
                    s.state = RootSyncState::Paused;
                }
            }
        }
        self.inner.events.publish(self.status());
    }

    /// Resume after a pause.
    pub fn resume(&self, root_id: Option<Uuid>) {
        match root_id {
            None => self
                .inner
                .paused
                .store(false, std::sync::atomic::Ordering::SeqCst),
            Some(id) => {
                if let Some(r) = self.inner.roots.lock().expect("roots lock").get_mut(&id) {
                    r.paused = false;
                }
                if let Some(s) = self
                    .inner
                    .live
                    .roots
                    .lock()
                    .expect("status lock")
                    .get_mut(&id)
                {
                    if s.state == RootSyncState::Paused {
                        s.state = RootSyncState::Idle;
                    }
                }
            }
        }
        self.inner.events.publish(self.status());
    }

    #[must_use]
    fn is_paused(&self) -> bool {
        self.inner.paused.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Reconcile every chosen, unpaused root against its peer once —
    /// what a scheduled driver calls on a timer. Errors are recorded
    /// per-root in the status (and retried next tick), never propagated,
    /// so one unreachable peer doesn't stop the others.
    pub async fn tick(&self) {
        if self.is_paused() {
            return;
        }
        let jobs: Vec<(Uuid, SyncServiceClient, bool)> = {
            let roots = self.inner.roots.lock().expect("roots lock");
            roots
                .iter()
                .map(|(id, r)| (*id, r.peer.clone(), r.paused))
                .collect()
        };
        for (root_id, peer, paused) in jobs {
            if paused {
                continue;
            }
            let observer = self.inner.live.clone();
            match reconcile_with_progress(&self.inner.backend, &peer, root_id, &observer).await {
                Ok(report) => {
                    if let Some(s) = self
                        .inner
                        .live
                        .roots
                        .lock()
                        .expect("status lock")
                        .get_mut(&root_id)
                    {
                        s.chunks_fetched = u64::from(report.chunks_fetched);
                        s.chunks_skipped = u64::from(report.chunks_skipped);
                    }
                }
                Err(e) => {
                    tracing::warn!(%root_id, error = %e, "files-daemon: pull failed");
                }
            }
            self.inner.events.publish(self.status());
        }
    }

    /// Hydrate one path on demand (issue #263).
    pub async fn hydrate(&self, root_id: Uuid, path: String) -> Result<()> {
        self.inner.backend.hydrate(root_id, path).await?;
        Ok(())
    }

    /// Checkpoint one synced root's live tree now.
    pub async fn checkpoint_now(&self, root_id: Uuid) -> Result<()> {
        self.inner.backend.checkpoint_now(root_id, None).await?;
        Ok(())
    }

    /// The daemon's whole status, snapshotting live per-file progress.
    #[must_use]
    pub fn status(&self) -> DaemonStatus {
        let identity = self.inner.identity.lock().expect("identity lock");
        let roots_cfg = self.inner.roots.lock().expect("roots lock");
        let live = self.inner.live.roots.lock().expect("status lock");
        let roots = roots_cfg
            .iter()
            .map(|(id, cfg)| {
                let rs = live.get(id);
                let state = if cfg.paused {
                    RootSyncState::Paused
                } else {
                    rs.map_or(RootSyncState::Idle, |s| s.state)
                };
                RootStatus {
                    root_id: *id,
                    name: cfg.name.clone(),
                    state,
                    slice: cfg.slice.clone(),
                    files: rs.map(|s| s.files.clone()).unwrap_or_default(),
                    chunks_fetched: rs.map_or(0, |s| s.chunks_fetched),
                    chunks_skipped: rs.map_or(0, |s| s.chunks_skipped),
                    last_synced_at: rs.and_then(|s| s.last_synced_at),
                    last_error: rs.and_then(|s| s.last_error.clone()),
                }
            })
            .collect();
        DaemonStatus {
            device_id: Some(identity.device_id),
            enrolled: identity.secret.is_some(),
            paused: self.is_paused(),
            roots,
        }
    }

    /// The hub the control service's `#[subscribe]` stream attaches to.
    #[must_use]
    pub fn events(&self) -> &EventHub {
        &self.inner.events
    }
}

/// Re-export the event hub type from the crate's `hub` shim so the
/// service layer and the daemon share one publisher.
pub use crate::hub::EventHub;
