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
    /// Every peer that serves this root — plural, and that is the
    /// point.
    ///
    /// It was one, which is correct for two machines and wrong for
    /// three: choosing a root against a second peer *replaced* the
    /// first, so a studio machine that had been told about a laptop and
    /// then about a server quietly stopped hearing from the laptop.
    /// Nothing said so; the root simply never saw that machine's work
    /// again.
    ///
    /// Any peer may hold the newest heads, so a tick asks all of them.
    /// Reconcile is idempotent and cheap when there is nothing new — a
    /// heads call and no transfer — so asking three machines costs
    /// three round trips, not three copies.
    peers: Vec<PeerLink>,
}

/// One peer a root is pulled from.
struct PeerLink {
    client: SyncServiceClient,
    /// Its endpoint id, when it was dialled by one.
    ///
    /// A client cannot be written to disk, so this is what a restart
    /// restores from — see [`Choices`] — and what a failed pull redials.
    /// `None` for a peer handed in by an embedder (a test, an in-process
    /// link), which has no address to come back to.
    endpoint: Option<String>,
}

/// The sync choices, as they survive a restart.
///
/// A background service that forgets what it was syncing when the
/// machine reboots is a background service that stops syncing, quietly,
/// at the least convenient moment. The choice is a decision a person
/// made — `storage.tier.authored` — so it belongs on disk.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Choices {
    roots: Vec<Choice>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Choice {
    root_id: Uuid,
    name: String,
    slice: Vec<String>,
    /// The peer to dial for it. A choice with no endpoint cannot be
    /// restored and is not written.
    peer: String,
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
    /// Paths this root's heads disagree about, as of the last pull.
    divergent: Vec<String>,
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
    /// Where adopted roots land, reported in the status.
    roots_dir: Mutex<std::path::PathBuf>,
    /// Where each root appears in the tree people are shown — see
    /// `set_place`. Keyed by root, and deliberately not the same thing
    /// as where its bytes are.
    places: Mutex<BTreeMap<Uuid, String>>,
    /// The capture running right now, if one is — what `status`
    /// reports so an hours-long read is legible while it happens.
    capturing: Mutex<Option<crate::model::CaptureProgress>>,
    /// Who made each root, for the roots this machine watched being
    /// made. Keyed by root; absent for one that arrived from a peer or
    /// predates the record.
    made_by: Mutex<BTreeMap<Uuid, crate::model::MadeBy>>,
    /// Roots mounted as filesystems, and the session keeping each
    /// alive — dropping one unmounts it.
    mounts: Mutex<BTreeMap<Uuid, (std::path::PathBuf, crate::mount::fuser_session::Session)>>,
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
        // A default the embedder overrides with `set_roots_dir`. Beside
        // the data dir rather than inside it, because "somewhere under
        // an application-support directory" is a poor place for a
        // person's projects and a worse one to have guessed silently.
        let data_dir_for_roots = data_dir.join("roots");
        Ok(Self {
            inner: Arc::new(DaemonInner {
                backend,
                identity: Mutex::new(identity),
                data_dir,
                endpoint: Mutex::new(None),
                roots_dir: Mutex::new(data_dir_for_roots.clone()),
                places: Mutex::new(BTreeMap::new()),
                capturing: Mutex::new(None),
                made_by: Mutex::new(BTreeMap::new()),
                mounts: Mutex::new(BTreeMap::new()),
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

        // And stop pulling from it — but only from *it*. A root synced
        // with three machines loses one peer here, not the root: the
        // work the other two are doing is none of this machine's
        // business to drop.
        {
            let mut roots = self.inner.roots.lock().expect("roots lock");
            for root in roots.values_mut() {
                root.peers
                    .retain(|p| p.endpoint.as_deref() != Some(endpoint_id));
            }
            // A root left with no peer is one nothing can update. It
            // stays on disk (dismissing is not deleting) and stops being
            // a sync choice, which is what the status should say.
            roots.retain(|_, root| !root.peers.is_empty());
        }
        self.save_choices();
        self.inner.events.publish(self.status());
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
        // Written down, because being *told* the org over the socket and
        // being *started* with it in the environment should not differ
        // after a reboot. Without this, an agent paired from the app
        // came back knowing nothing about the org — the same class of
        // quiet forgetting as the sync choices.
        let path = self.inner.data_dir.join("coordinator");
        if let Err(e) = std::fs::write(&path, endpoint_id) {
            tracing::warn!(error = %e, "could not record the coordinator");
        }
        Ok(())
    }

    /// The org this agent was last told to sync with, if any.
    ///
    /// The environment wins when both are set: a service unit is an
    /// operator's explicit configuration, and a file this wrote is a
    /// memory of what somebody chose in the app.
    #[must_use]
    pub fn remembered_coordinator(&self) -> Option<String> {
        std::fs::read_to_string(self.inner.data_dir.join("coordinator"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
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

    /// Replace a root's peer connection with a fresh one.
    ///
    /// Best effort by design: a peer that is genuinely gone (shut lid,
    /// no network) fails here too, and the right response is to try
    /// again next tick rather than to drop a choice the person made.
    async fn redial(&self, root_id: Uuid, endpoint: &str) {
        match self.dial(endpoint).await {
            Ok(fresh) => {
                if let Some(link) = self
                    .inner
                    .roots
                    .lock()
                    .expect("roots lock")
                    .get_mut(&root_id)
                    .and_then(|root| {
                        root.peers
                            .iter_mut()
                            .find(|p| p.endpoint.as_deref() == Some(endpoint))
                    })
                {
                    link.client = fresh;
                }
                tracing::info!(%root_id, peer = %endpoint, "files-daemon: reconnected to the peer");
            }
            Err(e) => {
                tracing::debug!(%root_id, peer = %endpoint, error = %e, "files-daemon: peer still unreachable");
            }
        }
    }

    // ── The cloud folder ───────────────────────────────────────────

    /// Mount a root's live tree so dehydrated files fetch on open.
    ///
    /// This is what makes the tree behave the way people expect a cloud
    /// folder to: everything is listed at its real size, and opening
    /// something this machine does not hold gets it rather than getting
    /// a placeholder. Selective sync decides what is resident; the
    /// mount decides what happens when that turns out to be wrong.
    pub async fn mount(&self, root_id: Uuid, mountpoint: &std::path::Path) -> Result<()> {
        // Mounting the same root twice is not two windows onto it, it is
        // one window and a leaked session: the map holds one entry per
        // root, so the second insert would drop the first handle and
        // unmount what somebody is looking at. Say so instead.
        if let Some((at, _)) = self.inner.mounts.lock().expect("mount lock").get(&root_id) {
            return Err(DaemonError::BadRequest(format!(
                "that root is already mounted at {}",
                at.display()
            )));
        }

        let tree = self
            .inner
            .backend
            .get_root(root_id)
            .await?
            .path
            .ok_or_else(|| {
                DaemonError::BadRequest(
                    "that root has no tree on this machine to mount".to_string(),
                )
            })?;

        let place = self.place_of(root_id, "");
        let session = crate::mount::mount(
            self,
            root_id,
            std::path::Path::new(&tree),
            mountpoint,
            &place,
        )?;
        self.inner
            .mounts
            .lock()
            .expect("mount lock")
            .insert(root_id, (mountpoint.to_path_buf(), session));
        self.save_mounts();
        Ok(())
    }

    /// Unmount a root. The tree stays exactly where it is on disk — a
    /// mount is a window onto it, not the thing itself.
    pub fn unmount(&self, root_id: Uuid) -> Result<()> {
        let gone = self
            .inner
            .mounts
            .lock()
            .expect("mount lock")
            .remove(&root_id);
        match gone {
            // Dropping the session unmounts.
            Some((at, session)) => {
                drop(session);
                tracing::info!(%root_id, mountpoint = %at.display(), "unmounted");
                self.save_mounts();
                Ok(())
            }
            None => Err(DaemonError::NotFound("that root is not mounted".into())),
        }
    }

    /// Where each mounted root is mounted.
    #[must_use]
    pub fn mounts(&self) -> Vec<(Uuid, std::path::PathBuf)> {
        self.inner
            .mounts
            .lock()
            .expect("mount lock")
            .iter()
            .map(|(id, (at, _))| (*id, at.clone()))
            .collect()
    }

    fn mounts_path(&self) -> std::path::PathBuf {
        self.inner.data_dir.join("mounts.json")
    }

    fn save_mounts(&self) {
        let mounts = crate::mount::Mounts {
            at: self
                .mounts()
                .into_iter()
                .map(|(root_id, mountpoint)| crate::mount::Mounted {
                    root_id,
                    mountpoint,
                })
                .collect(),
        };
        match serde_json::to_vec_pretty(&mounts) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(self.mounts_path(), bytes) {
                    tracing::warn!(error = %e, "could not record the mounts");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize the mounts"),
        }
    }

    /// Re-mount what was mounted before the machine restarted.
    ///
    /// A mount is a decision somebody made, and a service that forgot it
    /// on reboot would leave them looking at an empty directory where
    /// their project used to be.
    pub async fn restore_mounts(&self) -> usize {
        let Ok(raw) = std::fs::read_to_string(self.mounts_path()) else {
            return 0;
        };
        let mounts: crate::mount::Mounts = match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "the recorded mounts are unreadable");
                return 0;
            }
        };
        let mut back = 0;
        for m in mounts.at {
            // The composed tree is not any one root, so it comes back
            // the way it went up — by composing every root again, which
            // also picks up anything shared since.
            if m.root_id == COMPOSED {
                let outcomes = self.mount_all(&m.mountpoint, false).await;
                let failed = outcomes.iter().filter(|(_, e)| e.is_some()).count();
                if failed == 0 {
                    back += 1;
                } else {
                    tracing::warn!(
                        at = %m.mountpoint.display(),
                        failed,
                        "could not bring the tree back"
                    );
                }
                continue;
            }
            match self.mount(m.root_id, &m.mountpoint).await {
                Ok(()) => back += 1,
                Err(e) => tracing::warn!(
                    root = %m.root_id,
                    at = %m.mountpoint.display(),
                    error = %e,
                    "could not re-mount this root"
                ),
            }
        }
        back
    }

    /// Every root this machine holds, shared or synced.
    pub async fn shares(&self) -> Result<Vec<files_proto::model::FileRootInfo>> {
        Ok(self.inner.backend.list_roots().await?)
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
        self.share_capturing(path, name, true).await
    }

    /// [`Self::share`], with a say over whether the first capture
    /// happens now.
    ///
    /// Registering a root is instant — it records a directory. Capturing
    /// it reads every byte to hash them, which on a 210 GB project over
    /// NFS is an hour during which nothing at all appears. Adopting an
    /// archive one project at a time that way means the last project
    /// shows up a day after the first.
    ///
    /// So the two are separable. `capture: false` registers the root and
    /// returns: it is listed, browsable, and mountable immediately, and
    /// its version history fills in when something checkpoints it.
    ///
    /// The default stays `true`, because the alternative is a root that
    /// looks shared and syncs as an empty tree — content reaches a peer
    /// from the store, so a root that has never been captured has
    /// nothing to send. A caller that defers is taking that on
    /// deliberately.
    pub async fn share_capturing(
        &self,
        path: &std::path::Path,
        name: Option<String>,
        capture: bool,
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
        if capture {
            self.inner.backend.checkpoint_now(root.id, None).await?;
        } else {
            // Remembered, because `capture` has no other way to know: a
            // root's repo exists the moment it is registered, so there
            // is nothing on disk that distinguishes "not captured yet"
            // from "captured and unchanged".
            self.remember_pending_capture(root.id);
        }
        // Watched from here on, so later edits are captured without
        // anyone asking — the same thing `start_capture` does for the
        // roots that already existed.
        self.inner.backend.watch_root(root.id)?;
        self.inner.events.publish(self.status());
        Ok(root)
    }

    // ── Where a root appears ───────────────────────────────────────

    /// Where `root_id` sits in the tree people are shown, as a relative
    /// path like `codywright/Projects/Some Record`.
    ///
    /// **Deliberately unrelated to where its bytes are.** A studio's
    /// disk is laid out by the accidents of fifteen years — an old
    /// server's export here, a rescued drive there, one client's work on
    /// a NAS because that is where the space was. The tree a person
    /// should see is not that, and reshaping six terabytes to make the
    /// two agree would be an expensive way to answer a question about
    /// presentation.
    ///
    /// So a root has a name, a path, and — separately — a place. Mounts
    /// compose the places into one tree; nothing moves.
    pub fn set_place(&self, root_id: Uuid, place: &str) -> Result<()> {
        let place = place.trim_matches('/').to_string();
        if place.is_empty() {
            return Err(DaemonError::BadRequest(
                "a place is a path in the tree, like `org/Projects/Name`".into(),
            ));
        }
        // A place that climbs out of the tree it is a place in would
        // mount a root anywhere on the disk, from a string that looks
        // like a folder name.
        if place.split('/').any(|part| part == ".." || part.is_empty()) {
            return Err(DaemonError::BadRequest(format!(
                "`{place}` is not a place inside the tree"
            )));
        }
        self.inner
            .places
            .lock()
            .expect("places lock")
            .insert(root_id, place);
        self.save_places();
        self.inner.events.publish(self.status());
        Ok(())
    }

    /// Where a root appears, or its name when nobody has said.
    #[must_use]
    pub fn place_of(&self, root_id: Uuid, name: &str) -> String {
        self.inner
            .places
            .lock()
            .expect("places lock")
            .get(&root_id)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    fn places_path(&self) -> std::path::PathBuf {
        self.inner.data_dir.join("places.json")
    }

    fn save_places(&self) {
        let places = self.inner.places.lock().expect("places lock").clone();
        match serde_json::to_vec_pretty(&places) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(self.places_path(), bytes) {
                    tracing::warn!(error = %e, "could not record where the roots appear");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize the places"),
        }
    }

    /// Read back where roots appear. Called at startup, beside the sync
    /// choices and the mounts, for the same reason: it is a decision
    /// somebody made.
    pub fn restore_places(&self) {
        let Ok(raw) = std::fs::read_to_string(self.places_path()) else {
            return;
        };
        match serde_json::from_str::<BTreeMap<Uuid, String>>(&raw) {
            Ok(places) => *self.inner.places.lock().expect("places lock") = places,
            Err(e) => tracing::warn!(error = %e, "the recorded places are unreadable"),
        }
    }

    /// Mount every root this machine holds, composed into one tree
    /// under `under`.
    ///
    /// This is the cloud folder as a person expects it: not one folder
    /// per project scattered wherever each was adopted, but a single
    /// tree — `<under>/<org>/Projects/<name>` — that looks the same on
    /// every machine regardless of which disk holds what.
    ///
    /// Returns what it mounted and what it could not, rather than the
    /// first error: one root with a bad path should not stop the other
    /// nine from appearing.
    pub async fn mount_all(
        &self,
        under: &std::path::Path,
        flat: bool,
    ) -> Vec<(String, Option<String>)> {
        let roots = match self.shares().await {
            Ok(roots) => roots,
            Err(e) => return vec![("(the roots)".into(), Some(e.to_string()))],
        };

        // Flattening can collide — two orgs with a project of the same
        // name land on one path. Counted first so the *pair* can be
        // disambiguated rather than whichever happened to be second.
        let mut taken: BTreeMap<String, usize> = BTreeMap::new();
        if flat {
            for root in &roots {
                *taken
                    .entry(flatten(&self.place_of(root.id, &root.name)))
                    .or_insert(0) += 1;
            }
        }

        let mut placed = Vec::new();
        let mut outcomes = Vec::new();
        for root in roots {
            let Some(tree) = root.path.clone() else {
                // Structure without content on this machine: real, and
                // nothing to show as a folder.
                continue;
            };
            let place = self.place_of(root.id, &root.name);
            let shown = if flat {
                let flattened = flatten(&place);
                if taken.get(&flattened).copied().unwrap_or(0) > 1 {
                    match place.split_once('/') {
                        Some((org, _)) => format!("{flattened} ({org})"),
                        None => flattened,
                    }
                } else {
                    flattened
                }
            } else {
                place
            };
            outcomes.push((shown.clone(), None));
            placed.push((root.id, std::path::PathBuf::from(tree), shown));
        }

        if placed.is_empty() {
            return outcomes;
        }

        // Everything the old per-root mounts left behind. One tree
        // replaces all of them, so anything still mounted is stale.
        for (id, at) in self.mounts() {
            if let Err(e) = self.unmount(id) {
                tracing::warn!(at = %at.display(), error = %e, "could not take down an old mount");
            }
        }

        let skeleton = self.inner.data_dir.join("tree");
        match crate::mount::mount_composed(self, &skeleton, placed, under) {
            Ok(session) => {
                self.inner
                    .mounts
                    .lock()
                    .expect("mount lock")
                    .insert(COMPOSED, (under.to_path_buf(), session));
                self.save_mounts();
            }
            Err(e) => {
                let why = e.to_string();
                for outcome in &mut outcomes {
                    outcome.1 = Some(why.clone());
                }
            }
        }
        outcomes
    }

    /// Capture every root that has never been captured, smallest first.
    ///
    /// The drain for [`Self::share_capturing`] with `capture: false`. An
    /// archive adopted that way is browsable at once and syncable as
    /// this works through it, which is the difference between a studio
    /// seeing its projects now and seeing them tomorrow.
    ///
    /// **Smallest first, deliberately.** Largest-first finishes the same
    /// total in the same time while showing nothing for the first hour;
    /// smallest-first has most of the projects ready early and leaves
    /// the one 210 GB session for last. Same work, and somebody can use
    /// it while it runs.
    ///
    /// Returns what it captured. Errors are per-root: a project that
    /// cannot be read should not stop the other thirty.
    pub async fn capture_pending(&self) -> Vec<(String, Option<String>)> {
        let roots = match self.shares().await {
            Ok(roots) => roots,
            Err(e) => return vec![("(the roots)".into(), Some(e.to_string()))],
        };

        // What was deferred, as the daemon recorded it when it deferred.
        //
        // Not "has no `.fts-files`", which was the first guess and is
        // wrong: registering a root creates its repo immediately, so
        // that test said every root had been captured and `capture`
        // did nothing at all. The agent is the one that chose to skip
        // the capture, so the agent is what should remember.
        let waiting = self.awaiting_capture();
        let mut pending = Vec::new();
        for root in roots {
            let Some(path) = root.path.clone() else {
                continue;
            };
            if !waiting.contains(&root.id) {
                continue;
            }
            let bytes = directory_bytes(std::path::Path::new(&path));
            pending.push((bytes, root.id, root.name));
        }
        pending.sort_by_key(|(bytes, _, _)| *bytes);

        let total = pending.len() as u32;
        let mut done = Vec::new();
        for (index, (bytes, id, name)) in pending.into_iter().enumerate() {
            *self.inner.capturing.lock().expect("capture lock") =
                Some(crate::model::CaptureProgress {
                    root: name.clone(),
                    done: index as u32 + 1,
                    total,
                    bytes,
                    since: Utc::now(),
                    file: String::new(),
                    files_done: 0,
                    files_total: 0,
                });
            self.inner.events.publish(self.status());

            // Per-file, from inside the capture: on a root with a
            // hundred thousand takes, "which one is it on" is the
            // difference between watching and guessing.
            let inner = Arc::clone(&self.inner);
            self.inner.backend.set_capture_progress(Some(Arc::new(
                move |path: &str, done: u64, total: u64| {
                    if let Some(p) = inner.capturing.lock().expect("capture lock").as_mut() {
                        p.file = path.to_string();
                        p.files_done = done;
                        p.files_total = total;
                    }
                },
            )));
            let error = self
                .inner
                .backend
                .checkpoint_now(id, None)
                .await
                .err()
                .map(|e| e.to_string());
            self.inner.backend.set_capture_progress(None);
            tracing::info!(root = %name, ok = error.is_none(), "captured a root");
            // Cleared one at a time, as each finishes. An archive takes
            // hours and a machine can be shut down in the middle of it;
            // clearing the whole list at the end would mean a reboot at
            // hour four starts again from nothing.
            if error.is_none() {
                self.forget_pending_capture(id);
            }
            done.push((name, error));
            self.inner.events.publish(self.status());
        }
        *self.inner.capturing.lock().expect("capture lock") = None;
        self.inner.events.publish(self.status());
        done
    }

    /// Start the backlog running in the agent, and return.
    ///
    /// Capture is measured in hours on an archive, so it does not belong
    /// on the far side of a request: a CLI that waits for it cannot be
    /// interrupted without killing it, and a caller that disconnects
    /// should not stop a machine reading its own disk. The work lives in
    /// the agent, and `status` is how anybody watches it.
    ///
    /// Refuses a second run rather than interleaving two passes over the
    /// same roots.
    pub fn start_capture_backlog(&self) -> Result<u32> {
        if self.inner.capturing.lock().expect("capture lock").is_some() {
            return Err(DaemonError::BadRequest(
                "a capture is already running — `status` shows where it is".into(),
            ));
        }
        let waiting = self.awaiting_capture().len() as u32;
        if waiting == 0 {
            return Ok(0);
        }
        let daemon = self.clone();
        tokio::spawn(async move {
            let done = daemon.capture_pending().await;
            let failed = done.iter().filter(|(_, e)| e.is_some()).count();
            tracing::info!(
                captured = done.len() - failed,
                failed,
                "capture backlog finished"
            );
        });
        Ok(waiting)
    }

    /// The roots registered without a first capture, still waiting for
    /// one. Empty on a machine that has never deferred.
    fn awaiting_capture(&self) -> std::collections::BTreeSet<Uuid> {
        std::fs::read_to_string(self.pending_capture_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn pending_capture_path(&self) -> std::path::PathBuf {
        self.inner.data_dir.join("awaiting-capture.json")
    }

    fn remember_pending_capture(&self, root_id: Uuid) {
        let mut waiting = self.awaiting_capture();
        waiting.insert(root_id);
        self.write_pending_capture(&waiting);
    }

    fn forget_pending_capture(&self, root_id: Uuid) {
        let mut waiting = self.awaiting_capture();
        waiting.remove(&root_id);
        self.write_pending_capture(&waiting);
    }

    fn write_pending_capture(&self, waiting: &std::collections::BTreeSet<Uuid>) {
        match serde_json::to_vec_pretty(waiting) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(self.pending_capture_path(), bytes) {
                    tracing::warn!(error = %e, "could not record what is awaiting capture");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize the capture backlog"),
        }
    }

    /// Stop holding a root: no longer served to peers, no longer
    /// pulled, and left on disk exactly as it is.
    ///
    /// The counterpart of [`Self::share`], and its absence was the same
    /// shape of hole as `forget`'s: a folder could be handed to the
    /// agent and never taken back, so a directory adopted by mistake
    /// stayed adopted, kept being offered to every admitted machine, and
    /// the only way out was editing the store by hand.
    ///
    /// Content is untouched. This is "stop tracking this", not "delete
    /// my project" — and the version history stays in the tree's own
    /// `.fts-files`, so re-sharing it later resumes rather than starts
    /// over.
    pub fn unshare(&self, root_id: Uuid) -> Result<()> {
        self.remove_sync_choice(root_id);
        self.inner.backend.forget_root(root_id)?;
        self.inner.events.publish(self.status());
        Ok(())
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
    ) -> Result<Vec<crate::service::Pulled>> {
        let mut outcomes = Vec::new();
        for root in self.peer_roots(endpoint_id).await? {
            let error = match self
                .sync_from_peer(endpoint_id, root.id, vec![], under)
                .await
            {
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!(root = %root.name, error = %e, "could not take this root");
                    Some(e.to_string())
                }
            };
            // Asked of the backend rather than assembled from `under`:
            // a root this machine already held kept the path it already
            // had, and only a newly adopted one lands under `under`.
            let path = self
                .inner
                .backend
                .get_root(root.id)
                .await
                .ok()
                .and_then(|r| r.path)
                .unwrap_or_default();
            outcomes.push(crate::service::Pulled {
                name: root.name,
                path,
                error,
            });
        }
        Ok(outcomes)
    }

    /// Machines this one has been told to sync with but has not managed
    /// to reach yet.
    ///
    /// "Sync with my laptop" is usually said while the laptop is shut —
    /// that is *why* it is being said. Before this, naming an
    /// unreachable machine failed with a dial timeout and recorded
    /// nothing, so the intent evaporated and had to be repeated once the
    /// other machine happened to be awake and somebody happened to
    /// remember.
    ///
    /// Persisted, because the same is true across a reboot, and cleared
    /// the moment the peer answers.
    fn pending_path(&self) -> std::path::PathBuf {
        self.inner.data_dir.join("pending-peers.json")
    }

    fn pending_peers(&self) -> Vec<String> {
        std::fs::read_to_string(self.pending_path())
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default()
    }

    fn set_pending_peers(&self, peers: &[String]) {
        let path = self.pending_path();
        if peers.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        match serde_json::to_vec_pretty(peers) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&path, bytes) {
                    tracing::warn!(error = %e, "could not record the peers still to reach");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize the pending peers"),
        }
    }

    /// Remember to sync with `endpoint_id` once it is reachable.
    pub fn remember_peer(&self, endpoint_id: &str) {
        let mut pending = self.pending_peers();
        if !pending.iter().any(|p| p == endpoint_id) {
            pending.push(endpoint_id.to_string());
            self.set_pending_peers(&pending);
        }
    }

    /// Try every machine this one is still waiting to reach.
    ///
    /// Runs on the tick, so a laptop that opens its lid is picked up
    /// within a cadence rather than when somebody thinks to re-run a
    /// command.
    async fn reach_pending(&self, under: &std::path::Path) {
        let pending = self.pending_peers();
        if pending.is_empty() {
            return;
        }
        let mut still_waiting = Vec::new();
        for peer in pending {
            match self.pull_all(&peer, under).await {
                Ok(taken) => {
                    tracing::info!(
                        peer = %peer,
                        roots = taken.len(),
                        "files-daemon: reached a machine we were waiting for"
                    );
                }
                Err(e) => {
                    tracing::debug!(peer = %peer, error = %e, "files-daemon: still cannot reach it");
                    still_waiting.push(peer);
                }
            }
        }
        self.set_pending_peers(&still_waiting);
    }

    /// Where this agent lands roots it adopts — reported in the status
    /// so a client never has to guess it.
    pub fn set_roots_dir(&self, dir: impl Into<std::path::PathBuf>) {
        *self.inner.roots_dir.lock().expect("roots dir lock") = dir.into();
    }

    /// Where this agent lands roots it adopts.
    #[must_use]
    pub fn roots_dir(&self) -> std::path::PathBuf {
        self.inner.roots_dir.lock().expect("roots dir lock").clone()
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
        // With the endpoint, so a restart dials this peer again rather
        // than coming back having quietly stopped syncing this root.
        self.set_sync_choice_from(
            root_id,
            &name,
            slice,
            peer,
            Some(endpoint_id.to_string()),
        )
        .await?;
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
        self.set_sync_choice_from(root_id, name, slice, peer, None)
            .await
    }

    /// [`Self::set_sync_choice`], remembering which endpoint served it
    /// so a restart can dial the same peer again.
    pub async fn set_sync_choice_from(
        &self,
        root_id: Uuid,
        name: &str,
        slice: Vec<String>,
        peer: SyncServiceClient,
        peer_endpoint: Option<String>,
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
        let entry = roots.entry(root_id).or_insert_with(|| SyncedRoot {
            name: name.to_string(),
            slice: slice.clone(),
            paused: false,
            peers: Vec::new(),
        });
        entry.name = name.to_string();
        entry.slice = slice;
        // Added, not replaced — unless it is the same machine again, in
        // which case the fresh client supersedes the old one (that is
        // what a redial is).
        match entry
            .peers
            .iter_mut()
            .find(|p| p.endpoint.is_some() && p.endpoint == peer_endpoint)
        {
            Some(existing) => existing.client = peer,
            None => entry.peers.push(PeerLink {
                client: peer,
                endpoint: peer_endpoint,
            }),
        }
        drop(roots);
        self.save_choices();
        self.inner.events.publish(self.status());
        Ok(())
    }

    /// Where the choices live.
    fn choices_path(&self) -> std::path::PathBuf {
        self.inner.data_dir.join("sync-choices.json")
    }

    /// Write the choices that can be restored.
    ///
    /// Only the ones with an endpoint: a choice against a peer handed in
    /// by an embedder has no address to dial on the way back, and
    /// recording a name we cannot act on would make a restart look like
    /// it had restored something.
    fn save_choices(&self) {
        let choices = Choices {
            roots: self
                .inner
                .roots
                .lock()
                .expect("roots lock")
                .iter()
                // One row per (root, peer), so a machine that syncs a
                // root with two others comes back syncing it with both.
                .flat_map(|(id, r)| {
                    r.peers
                        .iter()
                        .filter_map(|p| p.endpoint.as_ref())
                        .map(|peer| Choice {
                            root_id: *id,
                            name: r.name.clone(),
                            slice: r.slice.clone(),
                            peer: peer.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
        };
        let path = self.choices_path();
        match serde_json::to_vec_pretty(&choices) {
            // Losing this costs "what was I syncing" across a restart,
            // not correctness now, so it warns rather than fails a call
            // the person asked for.
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&path, bytes) {
                    tracing::warn!(path = %path.display(), error = %e, "could not record the sync choices");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize the sync choices"),
        }
    }

    /// Re-establish the choices this machine had before it restarted.
    ///
    /// Dials each remembered peer; one that is unreachable is skipped
    /// with a warning rather than dropped, because a laptop that is shut
    /// right now is the ordinary case and its choice is still the
    /// person's. Returns how many came back.
    pub async fn restore_choices(&self) -> usize {
        let path = self.choices_path();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return 0;
        };
        let choices: Choices = match serde_json::from_str(&raw) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "sync choices unreadable");
                return 0;
            }
        };
        let mut restored = 0;
        for choice in choices.roots {
            match self.dial(&choice.peer).await {
                Ok(peer) => {
                    if let Err(e) = self
                        .set_sync_choice_from(
                            choice.root_id,
                            &choice.name,
                            choice.slice.clone(),
                            peer,
                            Some(choice.peer.clone()),
                        )
                        .await
                    {
                        tracing::warn!(root = %choice.name, error = %e, "could not resume this root");
                    } else {
                        restored += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        root = %choice.name,
                        peer = %choice.peer,
                        error = %e,
                        "peer unreachable — will retry on the next restart"
                    );
                }
            }
        }
        if restored > 0 {
            tracing::info!(restored, "resumed syncing");
        }
        restored
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
        // Persisted too, or "stop syncing this" lasts until the next
        // restart brings it back.
        self.save_choices();
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

        // Machines somebody named while they were asleep. Cheap when
        // there are none, which is the ordinary case.
        let under = self.roots_dir();
        self.reach_pending(&under).await;
        // One job per (root, peer): any peer may hold the newest heads,
        // so a machine that syncs with three others asks all three.
        let jobs: Vec<(Uuid, SyncServiceClient, Option<String>, bool)> = {
            let roots = self.inner.roots.lock().expect("roots lock");
            roots
                .iter()
                .flat_map(|(id, r)| {
                    r.peers
                        .iter()
                        .map(|p| (*id, p.client.clone(), p.endpoint.clone(), r.paused))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        for (root_id, peer, peer_endpoint, paused) in jobs {
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
                    // A client is a live connection, and the peer at the
                    // other end restarts, sleeps, changes networks. The
                    // stored client is then dead for good: every tick
                    // fails with "vox connection closed" and the root
                    // sits in Error until *this* machine is restarted —
                    // which is a background service that stops working
                    // because the other one was upgraded.
                    //
                    // So a failed pull redials, and the next tick uses
                    // the fresh connection. Only when the peer was
                    // reached by an endpoint id; one handed in by an
                    // embedder has no address to redial.
                    if let Some(endpoint) = peer_endpoint.as_deref() {
                        self.redial(root_id, endpoint).await;
                    }
                }
            }

            // What the two sides disagree about, after the pull that
            // could have introduced it. Cheap when there is nothing to
            // say — a root with one visible head returns immediately
            // without walking a tree — and the only moment this can
            // change is right here, so it is not worth a surface of its
            // own.
            let divergent: Vec<String> = self
                .inner
                .backend
                .divergences(root_id)
                .await
                .map(|d| d.into_iter().map(|info| info.path.to_string()).collect())
                .unwrap_or_default();
            if !divergent.is_empty() {
                tracing::warn!(
                    %root_id,
                    paths = divergent.len(),
                    "files-daemon: two machines changed the same files — waiting for a decision"
                );
            }
            if let Some(s) = self
                .inner
                .live
                .roots
                .lock()
                .expect("status lock")
                .get_mut(&root_id)
            {
                s.divergent = divergent;
            }
            self.inner.events.publish(self.status());
        }
    }

    /// Settle one divergent path by keeping every side.
    ///
    /// Only `KeepBoth` is offered here, and the restraint is the point:
    /// a person at a terminal, told two machines disagree about a file,
    /// cannot see either version from there. Picking one would be
    /// choosing which work to stop showing on the strength of a path
    /// name. Keeping both puts each side on the disk under its own name
    /// (`<stem> (divergent n).<ext>`), where the file can be opened and
    /// the real decision made by whoever knows what is in it — which is
    /// the app's job, and `resolve_divergence`'s `Pick` is how it does
    /// it.
    pub async fn keep_both(&self, root_id: Uuid, path: String) -> Result<()> {
        self.inner
            .backend
            .resolve_divergence(root_id, path, files_proto::model::DivergenceChoice::KeepBoth)
            .await?;
        self.inner.events.publish(self.status());
        Ok(())
    }

    /// Hydrate one path on demand (issue #263).
    pub async fn hydrate(&self, root_id: Uuid, path: String) -> Result<()> {
        self.inner.backend.hydrate(root_id, path).await?;
        Ok(())
    }

    /// Keep only what matches resident; everything else becomes a stub.
    ///
    /// The shape a studio actually wants from a machine that is not the
    /// one holding the masters: a record's Ogg stems are seven hundred
    /// megabytes and its WAVs are eight gigabytes, so a laptop carries
    /// `**/ogg/**` and lets the rest sit as stubs — listed at full size,
    /// one open away, fetched when something actually needs them.
    ///
    /// Two steps, because they are two different questions. The policy
    /// is *stored*, so it survives and governs every later materialize:
    /// content arriving from a peer lands resident or as a stub
    /// according to it, without a second pass. Applying it is what acts
    /// on the files already here.
    ///
    /// Nothing is lost either way. A dehydrated file's content is in the
    /// store and on the peers, and the pass refuses to dehydrate
    /// anything whose bytes differ from the last checkpoint — work in
    /// progress is never traded for disk.
    pub async fn keep_only(
        &self,
        root_id: Uuid,
        patterns: Vec<String>,
    ) -> Result<crate::service::KeptReport> {
        self.inner
            .backend
            .set_hydration_policy(root_id, patterns)
            .await?;
        let report = self.inner.backend.apply_hydration_policy(root_id).await?;
        self.inner.events.publish(self.status());
        Ok(crate::service::KeptReport {
            hydrated: report.hydrated.len() as u32,
            dehydrated: report.dehydrated.len() as u32,
            skipped_dirty: report.skipped_dirty.len() as u32,
            failed: report.failed.len() as u32,
        })
    }

    /// Record who made a root, and when.
    ///
    /// Written the moment a folder becomes a project, because that is
    /// the only moment the answer is free: afterwards nothing on disk
    /// says who created a directory that has since been written to by
    /// half a studio.
    pub fn made_by(&self, root_id: Uuid, who: crate::model::MadeBy) {
        self.inner
            .made_by
            .lock()
            .expect("made-by lock")
            .insert(root_id, who);
        self.save_made_by();
        self.inner.events.publish(self.status());
    }

    /// Who made a root, if this machine saw it happen.
    #[must_use]
    pub fn maker_of(&self, root_id: Uuid) -> Option<crate::model::MadeBy> {
        self.inner
            .made_by
            .lock()
            .expect("made-by lock")
            .get(&root_id)
            .cloned()
    }

    fn made_by_path(&self) -> std::path::PathBuf {
        self.inner.data_dir.join("made-by.json")
    }

    fn save_made_by(&self) {
        let all = self.inner.made_by.lock().expect("made-by lock").clone();
        match serde_json::to_vec_pretty(&all) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(self.made_by_path(), bytes) {
                    tracing::warn!(error = %e, "could not record who made a root");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize the makers"),
        }
    }

    /// Read back who made what. Beside the places, and for the same
    /// reason: it is a fact about the project, not about this process.
    pub fn restore_made_by(&self) {
        let Ok(raw) = std::fs::read_to_string(self.made_by_path()) else {
            return;
        };
        match serde_json::from_str::<BTreeMap<Uuid, crate::model::MadeBy>>(&raw) {
            Ok(all) => *self.inner.made_by.lock().expect("made-by lock") = all,
            Err(e) => tracing::warn!(error = %e, "the recorded makers are unreadable"),
        }
    }

    /// The patterns this root keeps resident, empty for "everything".
    pub async fn kept(&self, root_id: Uuid) -> Result<Vec<String>> {
        Ok(self.inner.backend.hydration_policy(root_id).await?)
    }

    /// Give a file's bytes back to the disk, leaving the file itself.
    ///
    /// The other half of the cloud folder, and the half that makes it
    /// worth having: a machine with a 500 GB disk can hold a 4 TB
    /// project as long as evicting what it is not working on is one
    /// call, and getting it back is opening the file. Nothing is lost —
    /// the content is in the version store and on the peers; what is
    /// released is the resident copy.
    pub async fn dehydrate(&self, root_id: Uuid, path: String) -> Result<()> {
        self.inner.backend.dehydrate(root_id, path).await?;
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
        let mounts = self.inner.mounts.lock().expect("mount lock");
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
                    // Every peer it pulls from: on a desk with three
                    // machines "where does this come from" has more than
                    // one answer, and showing the first would be picking
                    // one arbitrarily.
                    peers: cfg
                        .peers
                        .iter()
                        .map(|p| p.endpoint.clone().unwrap_or_else(|| "(direct)".into()))
                        .collect(),
                    state,
                    slice: cfg.slice.clone(),
                    files: rs.map(|s| s.files.clone()).unwrap_or_default(),
                    divergent: rs.map(|s| s.divergent.clone()).unwrap_or_default(),
                    chunks_fetched: rs.map_or(0, |s| s.chunks_fetched),
                    chunks_skipped: rs.map_or(0, |s| s.chunks_skipped),
                    last_synced_at: rs.and_then(|s| s.last_synced_at),
                    last_error: rs.and_then(|s| s.last_error.clone()),
                    mounted_at: mounts
                        .get(id)
                        .map(|(at, _)| at.to_string_lossy().into_owned()),
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
            roots_dir: self.inner.roots_dir.lock().expect("roots dir lock").to_string_lossy().into_owned(),
            capturing: self.inner.capturing.lock().expect("capture lock").clone(),
            awaiting_capture: self.awaiting_capture().len() as u32,
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

/// The composed tree's id in the mount map.
///
/// One mount covers every root, so it is not any root's id — a fixed
/// one, so a restart re-mounts the tree rather than treating it as a
/// root that has gone away.
const COMPOSED: Uuid = Uuid::from_u128(0x7a5c_0000_0000_0000_0000_0000_0000_0001);

/// Rough size of a directory tree, for ordering work by it.
///
/// Deliberately cheap and deliberately approximate: it is deciding what
/// to hash first, and a `stat` walk that is wrong by a few percent picks
/// the same order as an exact one. Unreadable entries are skipped rather
/// than failing — a size used for sorting must never be the thing that
/// stops the work.
fn directory_bytes(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// A place with its org dropped: `tombrooks/Projects/X` → `Projects/X`.
///
/// The org is how the work is *stored* — whose it is, who is billed for
/// it, who may see it. It is not how somebody looks for it. Asked to
/// find a session from March, nobody first remembers which client it was
/// under; they remember the name. Grouping by org makes that a search
/// across six folders instead of a glance at one.
///
/// So the same roots compose two ways from the same places, and neither
/// is a copy: by org when the question is "whose is this", flat when the
/// question is "where is that session".
///
/// A place with no org — `Assets`, shared across all of them — is
/// already flat and is left alone.
fn flatten(place: &str) -> String {
    match place.split_once('/') {
        Some((_org, rest)) => rest.to_string(),
        None => place.to_string(),
    }
}
