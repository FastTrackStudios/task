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
    /// This machine's endpoint: its address, its identity, and the thing
    /// it serves its own replica lane on. `None` until
    /// [`SyncDaemon::bind_peering`] — a daemon can pull without one, and
    /// cannot be pulled from without one.
    endpoint: Mutex<Option<architect::iroh_link::iroh::Endpoint>>,
    /// The boundary this machine's shared folders are recorded in —
    /// see `SyncDaemon::share`.
    shared: Mutex<Option<Arc<crate::peering::DeviceRoots>>>,
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
                endpoint: Mutex::new(None),
                shared: Mutex::new(None),
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

    // ── The device as a peer ───────────────────────────────────────
    //
    // Everything above this line is the pulling half. What follows is
    // the half that makes sync bidirectional in a deployment rather
    // than only in a test: a machine nobody can dial has no way to hand
    // over the work it did offline, because the engine has no push.

    // t[impl files.topology.multi-server] — the device half: a machine
    // that serves its own replica lane can be reached directly by
    // another, so "bytes move directly over iroh/QUIC where two peers
    // can reach each other" stops requiring one of them to be a server
    /// Bind this machine's endpoint and start serving the replica lane
    /// on it.
    ///
    /// Returns the endpoint id — the address to admit on the other side.
    /// Idempotent in effect: binding twice would mint a second address
    /// for one machine, so a daemon that already has an endpoint returns
    /// the one it has.
    pub async fn bind_peering(&self, book: Option<files::AddressBook>) -> Result<String> {
        if let Some(id) = self.endpoint_id() {
            return Ok(id);
        }
        let key = DeviceIdentity::endpoint_key(&self.inner.data_dir)?;
        let endpoint = crate::peering::bind(key, book).await?;
        Ok(self.attach_endpoint(endpoint))
    }

    /// Serve on an endpoint bound by the caller — the embedder's door
    /// (the desktop app binds its own, as does the integration suite,
    /// which seeds an address book to resolve ids with no internet).
    pub fn attach_endpoint(&self, endpoint: architect::iroh_link::iroh::Endpoint) -> String {
        let id = endpoint.id().to_string();
        *self.inner.endpoint.lock().expect("endpoint lock") = Some(endpoint.clone());
        let backend = self.inner.backend.clone();
        let whose = format!("device {}", self.device_id());
        tokio::spawn(async move {
            crate::peering::serve(backend, whose, endpoint).await;
        });
        tracing::info!(endpoint_id = %id, "files-daemon: serving the replica lane");
        id
    }

    /// This machine's endpoint id, once bound.
    #[must_use]
    pub fn endpoint_id(&self) -> Option<String> {
        self.inner
            .endpoint
            .lock()
            .expect("endpoint lock")
            .as_ref()
            .map(|e| e.id().to_string())
    }

    fn endpoint(&self) -> Result<architect::iroh_link::iroh::Endpoint> {
        self.inner
            .endpoint
            .lock()
            .expect("endpoint lock")
            .clone()
            .ok_or_else(|| {
                DaemonError::BadRequest(
                    "this daemon has no endpoint — call bind_peering first".into(),
                )
            })
    }

    /// Admit a peer to this machine's replica lane.
    ///
    /// The symmetric half of the org admitting this device: a server
    /// that is going to *pull* this laptop's offline checkpoints has to
    /// be on the laptop's own list, because the laptop's gate knows
    /// nothing but endpoint ids.
    pub fn admit_peer(&self, endpoint_id: &str) {
        self.inner.backend.admit_host(
            files_domain::HostId(endpoint_id.to_string()),
            // A device's peers hold the whole thing, structure and
            // content alike; a structure-only relationship is a server
            // arrangement and not something a laptop hands out.
            files_domain::Hosting::working(),
        );
    }

    /// Stop admitting a peer. Takes effect on its next call.
    pub fn dismiss_peer(&self, endpoint_id: &str) {
        self.inner
            .backend
            .dismiss_host(&files_domain::HostId(endpoint_id.to_string()));
    }

    /// Every peer this machine admits.
    #[must_use]
    pub fn peers(&self) -> Vec<String> {
        self.inner
            .backend
            .admitted_hosts()
            .into_iter()
            .map(|(h, _)| h.0)
            .collect()
    }

    /// Open the replica lane on `endpoint_id`.
    pub async fn dial(&self, endpoint_id: &str) -> Result<SyncServiceClient> {
        crate::peering::dial(&self.endpoint()?, endpoint_id).await
    }

    /// Dial `endpoint_id` and keep it as the coordinator this daemon
    /// pulls from — the deployed replacement for handing the binary a
    /// `ws://` URL, and the reason enrollment needs no token: the org
    /// admitted this device's endpoint, and the handshake proves it.
    pub async fn set_coordinator_peer(&self, endpoint_id: &str) -> Result<()> {
        let peer = self.dial(endpoint_id).await?;
        self.set_coordinator(peer);
        Ok(())
    }

    /// Hold the boundary this daemon's shared folders are recorded in,
    /// so [`Self::share`] can widen it.
    ///
    /// Optional because an embedder may manage the backend's boundaries
    /// itself; a daemon without one can still sync everything it was
    /// given, it just cannot be told to share a new folder.
    pub fn with_shared_dirs(&self, dirs: Arc<crate::peering::DeviceRoots>) {
        *self.inner.shared.lock().expect("shared dirs lock") = Some(dirs);
    }

    /// Share a folder from this machine: version it, checkpoint it, and
    /// serve it to admitted peers.
    ///
    /// This is the other half of a two-machine setup and the half that
    /// was missing. Everything else here syncs roots that came from
    /// somewhere else; a person with a project on *this* disk had no way
    /// to say so — the agent would happily serve a replica lane holding
    /// nothing anyone asked it to hold.
    ///
    /// The checkpoint is not a nicety: content reaches a peer from the
    /// store, so a root that has never been captured is a root that
    /// syncs as an empty tree.
    pub async fn share(
        &self,
        path: &std::path::Path,
        name: Option<String>,
    ) -> Result<files_proto::model::FileRootInfo> {
        let path = path
            .canonicalize()
            .map_err(|e| DaemonError::Io(format!("{}: {e}", path.display())))?;
        if !path.is_dir() {
            return Err(DaemonError::BadRequest(format!(
                "{} is not a directory — a File Root is a folder",
                path.display()
            )));
        }
        // Widen the boundary before adopting inside it, or the backend
        // refuses the very folder this call exists to accept.
        if let Some(dirs) = self.inner.shared.lock().expect("shared dirs lock").clone() {
            dirs.permit(&path)?;
        }
        let name = name.unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        });
        let root = self
            .inner
            .backend
            .create_root(
                path.to_string_lossy().into_owned(),
                name,
                files_proto::model::RootFlavor::Media,
            )
            .await?;
        self.inner.backend.checkpoint_now(root.id, None).await?;
        // Watched from here on, so later edits are captured without
        // anyone asking — the same thing `start_capture` does for the
        // roots that already existed.
        self.inner.backend.watch_root(root.id)?;
        self.inner.events.publish(self.status());
        Ok(root)
    }

    /// Sync everything `endpoint_id` offers, adopting what this machine
    /// does not have under `under`.
    ///
    /// The device-to-device shape: two laptops, no server, each holding
    /// what the other shared.
    pub async fn pull_all(
        &self,
        endpoint_id: &str,
        under: &std::path::Path,
    ) -> Result<Vec<String>> {
        let mut taken = Vec::new();
        for root in self.peer_roots(endpoint_id).await? {
            match self
                .sync_from_peer(endpoint_id, root.id, vec![], under)
                .await
            {
                Ok(_) => taken.push(root.name),
                Err(e) => tracing::warn!(root = %root.name, error = %e, "could not take this root"),
            }
        }
        Ok(taken)
    }

    /// What `endpoint_id` holds — the "what have you got" a fresh
    /// machine starts from.
    pub async fn peer_roots(&self, endpoint_id: &str) -> Result<Vec<files_sync::WireRoot>> {
        let peer = self.dial(endpoint_id).await?;
        Self::remote_roots(&peer).await
    }

    /// One place the `roots` RPC's transport error becomes a
    /// [`DaemonError`] — a vox error is a transport failure, so it is
    /// `Io` rather than anything the caller can act on differently.
    async fn remote_roots(peer: &SyncServiceClient) -> Result<Vec<files_sync::WireRoot>> {
        peer.roots()
            .await
            .map_err(|e| DaemonError::Io(format!("roots rpc: {e}")))
    }

    /// Sync `root_id` from `endpoint_id`, adopting it locally first if
    /// this machine has never seen it.
    ///
    /// The adoption is what makes this usable on a new machine. A root
    /// the daemon does not hold cannot be reconciled into nothing —
    /// `adopt_replica` is what gives the pull an id, a name, a flavor
    /// and somewhere on this disk to land, and `under` is where the
    /// caller says which disk.
    pub async fn sync_from_peer(
        &self,
        endpoint_id: &str,
        root_id: Uuid,
        slice: Vec<String>,
        under: &std::path::Path,
    ) -> Result<DaemonStatus> {
        let peer = self.dial(endpoint_id).await?;
        let name = match self.inner.backend.get_root(root_id).await {
            Ok(local) => local.name,
            Err(_) => {
                let remote = Self::remote_roots(&peer)
                    .await?
                    .into_iter()
                    .find(|r| r.id == root_id)
                    .ok_or_else(|| {
                        DaemonError::NotFound(format!("{endpoint_id} holds no root {root_id}"))
                    })?;
                let tree = under.join(&remote.name);
                std::fs::create_dir_all(&tree).map_err(|e| DaemonError::Io(e.to_string()))?;
                self.inner.backend.adopt_replica(
                    root_id,
                    &remote.name,
                    tree.to_str().ok_or_else(|| {
                        DaemonError::BadRequest(format!("{} is not utf-8", tree.display()))
                    })?,
                    remote.flavor,
                )?;
                remote.name
            }
        };
        self.set_sync_choice(root_id, &name, slice, peer).await?;
        Ok(self.status())
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
    /// Start watching every root on this machine, so local edits become
    /// checkpoints without anyone asking.
    ///
    /// This is what a server does at startup (`enable_watching`), and a
    /// device needs it more, not less: on a server the work arrives
    /// through the write path, and on a laptop it arrives through Pro
    /// Tools writing a session file that nothing told us about. Without
    /// it a daemon's local changes were never captured, so "sync both
    /// ways" had nothing to send in one of the directions — the pull was
    /// working perfectly against a history that never moved.
    ///
    /// The watcher only *hints*; [`Self::tick`] runs the cadence pass
    /// that decides when a hint becomes a capture.
    pub async fn start_capture(&self) {
        self.inner.backend.enable_watching().await;
    }

    pub async fn tick(&self) {
        if self.is_paused() {
            return;
        }
        // Capture before pulling. The order matters on the machine that
        // has been offline: its own work becomes a commit first, so the
        // reconcile that follows brings the other side's line into a
        // store that already holds this one — which is what makes the
        // two lines siblings to be resolved rather than one silently
        // overwriting the other on the next materialize.
        let captured = self.inner.backend.tick().await;
        if !captured.is_empty() {
            tracing::debug!(count = captured.len(), "files-daemon: captured local work");
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
            endpoint_id: self
                .inner
                .endpoint
                .lock()
                .expect("endpoint lock")
                .as_ref()
                .map(|e| e.id().to_string()),
            peers: self
                .inner
                .backend
                .admitted_hosts()
                .into_iter()
                .map(|(h, _)| h.0)
                .collect(),
            coordinator: self
                .inner
                .coordinator
                .lock()
                .expect("coordinator lock")
                .is_some(),
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
