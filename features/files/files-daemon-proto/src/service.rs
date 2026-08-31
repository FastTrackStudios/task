//! [`DaemonControlService`] — the RPC the sync daemon serves on a
//! **local socket** (issue #265): the desktop app and the CLI are both
//! just clients of it, so status and control look the same from either.
//! Not a network surface — it governs the local machine's own daemon,
//! authenticated by local-socket access (the device credential is what
//! authenticates the daemon to the *coordinator*, a separate axis).

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::DaemonStatus;

/// The Files fault, as this surface reports it.
///
/// Lives here rather than in `files-daemon` because [`DaemonError`]
/// does: an orphan impl cannot be written on the far side of a crate
/// boundary, and the alternative — every `?` in the agent replaced by a
/// mapping call — pays for the split in noise at hundreds of call sites.
/// `files-proto` is a wire contract like this one, so the dependency
/// costs a client nothing.
impl From<files_proto::FilesError> for DaemonError {
    fn from(e: files_proto::FilesError) -> Self {
        match e {
            files_proto::FilesError::NotFound(m) => DaemonError::NotFound(m),
            files_proto::FilesError::AlreadyExists(m) | files_proto::FilesError::BadRequest(m) => {
                DaemonError::BadRequest(m)
            }
            files_proto::FilesError::Io(m) => DaemonError::Io(m),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum DaemonError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not enrolled: {0}")]
    NotEnrolled(String),
    #[error("io: {0}")]
    Io(String),
}

/// A root, and where it appears in the tree people are shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct PlacedRoot {
    pub id: Uuid,
    pub name: String,
    /// Its live tree on this machine, empty when it holds structure
    /// without content.
    pub path: String,
    /// Where it appears — `codywright/Projects/Some Record`. Falls back
    /// to the name for a root nobody has placed.
    pub place: String,
}

/// What a `keep_only` pass did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct KeptReport {
    /// Files brought resident because they match and were stubs.
    pub hydrated: u32,
    /// Files turned into stubs because they do not match.
    pub dehydrated: u32,
    /// Files left alone because their bytes differ from the last
    /// checkpoint. Work in progress is never traded for disk space, and
    /// a count here is worth seeing rather than assuming.
    pub skipped_dirty: u32,
    /// Files the pass could not act on.
    pub failed: u32,
}

/// What became of one root a peer offered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Pulled {
    pub name: String,
    /// Where it lives on this machine.
    ///
    /// Not `<roots dir>/<name>`, which is only where a *new* root
    /// lands: one this machine already held stays where it already is,
    /// and printing the would-be path told a person their project had
    /// moved when it had not.
    pub path: String,
    /// Why it was not taken, if it was not.
    pub error: Option<String>,
}

/// Live status changes pushed to a subscriber — the app's status panel
/// folds these in rather than polling [`DaemonControlService::status`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum DaemonEvent {
    /// The whole status changed enough to re-read (a pull started or
    /// finished, a choice changed, pause toggled). The payload is the
    /// fresh snapshot so a subscriber needs no follow-up call.
    Changed(DaemonStatus),
}

#[architect::rpc]
pub trait DaemonControlService {
    /// The daemon's whole state, including per-file progress for every
    /// pull in flight (the information a user wants about a sync, not a
    /// bare "syncing").
    async fn status(&self) -> Result<DaemonStatus, DaemonError>;

    /// Start syncing `root_id` with an optional selective-sync `slice`
    /// (gitignore-dialect patterns; empty = the whole root). Re-calling
    /// updates the slice. The daemon pulls it on the next tick.
    async fn set_sync_choice(
        &self,
        root_id: Uuid,
        slice: Vec<String>,
    ) -> Result<DaemonStatus, DaemonError>;

    /// Stop syncing `root_id` — drops it from the choice set. Local
    /// content stays; nothing is deleted.
    async fn remove_sync_choice(&self, root_id: Uuid) -> Result<DaemonStatus, DaemonError>;

    /// Every root this machine holds — what it serves to admitted peers,
    /// whether it originated here or arrived from one.
    ///
    /// Distinct from the roots in [`crate::model::DaemonStatus`], which
    /// are the ones this machine *pulls*. A folder shared from here is
    /// not pulled from anywhere, so it appeared in no listing at all:
    /// the agent would serve it perfectly and be unable to say it had
    /// it, and `checkpoint <name>` could not resolve the name.
    async fn shares(&self) -> Result<Vec<(Uuid, String, String)>, DaemonError>;

    /// Every root and where it appears in the composed tree — id, name,
    /// live tree on disk, place.
    ///
    /// [`Self::shares`] without the place, which is the one thing a
    /// client needs to build the tree a person sees rather than a flat
    /// list of forty-six folders. Separate from `shares` so an older
    /// client's decode plan is untouched.
    async fn placed_roots(&self) -> Result<Vec<PlacedRoot>, DaemonError>;

    /// Share a folder without capturing it yet.
    ///
    /// Registering a root is instant; capturing reads every byte. An
    /// archive adopted with `capture: false` is listed and browsable at
    /// once, and [`Self::capture_pending`] fills the history in.
    async fn share_deferred(
        &self,
        path: String,
        name: Option<String>,
    ) -> Result<files_proto::model::FileRootInfo, DaemonError>;

    /// Start reading the backlog, and return the number of roots it
    /// will work through.
    ///
    /// The work runs *in the agent*, smallest root first, because it is
    /// measured in hours on an archive: a caller that waited for it
    /// could not be interrupted without killing it, and a disconnect
    /// should not stop a machine reading its own disk.
    /// [`crate::model::DaemonStatus::capturing`] is how anybody watches.
    async fn start_capture(&self) -> Result<u32, DaemonError>;

    /// Share a folder from this machine: version it, checkpoint it, and
    /// serve it to admitted peers. Returns the root's id and name.
    ///
    /// Without this the agent could only ever hold what somebody else
    /// already held — fine for a laptop taking an org's projects, and
    /// useless for two machines that just want the same folder.
    async fn share(&self, path: String, name: Option<String>)
    -> Result<(Uuid, String), DaemonError>;

    /// Stop holding a root: no longer served, no longer pulled, and
    /// left on disk exactly as it is. "Stop tracking this", never
    /// "delete my project".
    async fn unshare(&self, root_id: Uuid) -> Result<(), DaemonError>;

    /// Take everything `endpoint_id` offers, adopting what this machine
    /// does not have under `under` (empty for this agent's own roots
    /// directory, which is nearly always what a caller wants — see
    /// [`crate::model::DaemonStatus::roots_dir`]).
    ///
    /// One outcome per root, failures included. Returning only the
    /// successes reads as "the peer had nothing", which is what it did
    /// read as while every adoption was being refused.
    async fn pull_all(
        &self,
        endpoint_id: String,
        under: String,
    ) -> Result<Vec<Pulled>, DaemonError>;

    /// Point the running agent at the org it syncs with.
    ///
    /// The alternative — and what pairing did first — is to re-run
    /// `install --coordinator`, which rewrites the service unit and
    /// restarts the agent. That is the right move on a machine with no
    /// service yet and much too heavy on one that is running: it
    /// interrupts transfers in flight to deliver a string the agent
    /// could simply have been told.
    async fn set_coordinator(&self, endpoint_id: String) -> Result<DaemonStatus, DaemonError>;

    /// Remember to sync with `endpoint_id` once it can be reached.
    ///
    /// "Sync with my laptop" is usually said while the laptop is shut —
    /// that is why it is being said. The intent is kept and retried on
    /// the tick instead of failing with a dial timeout and evaporating.
    async fn remember_peer(&self, endpoint_id: String) -> Result<(), DaemonError>;

    /// Admit `endpoint_id` to this machine's own replica lane.
    ///
    /// Sync is two pulls, so being *pullable* is half of syncing: a
    /// server that has admitted this device still cannot collect the
    /// work it did offline until this device admits the server back.
    async fn admit_peer(&self, endpoint_id: String) -> Result<DaemonStatus, DaemonError>;

    /// Stop admitting `endpoint_id`. Takes effect on its next call,
    /// not on its agreement.
    async fn dismiss_peer(&self, endpoint_id: String) -> Result<DaemonStatus, DaemonError>;

    /// What `endpoint_id` holds, as `(root id, name)` — "what have you
    /// got", which is where a machine that holds nothing has to start.
    async fn peer_roots(&self, endpoint_id: String)
    -> Result<Vec<(Uuid, String)>, DaemonError>;

    /// Sync `root_id` from `endpoint_id`, adopting it under `under` if
    /// this machine has never seen it. `slice` is the selective-sync
    /// choice (empty = the whole root).
    async fn sync_from_peer(
        &self,
        endpoint_id: String,
        root_id: Uuid,
        slice: Vec<String>,
        under: String,
    ) -> Result<DaemonStatus, DaemonError>;

    /// Pause all syncing (or one root when `root_id` is set). No pulls
    /// run until resumed.
    async fn pause(&self, root_id: Option<Uuid>) -> Result<DaemonStatus, DaemonError>;

    /// Resume after a [`DaemonControlService::pause`].
    async fn resume(&self, root_id: Option<Uuid>) -> Result<DaemonStatus, DaemonError>;

    /// Hydrate one path in a synced root now (glossary "Pointer stub":
    /// pull its content resident on demand). Fails if its content is
    /// not yet local — a pull brings it first.
    async fn hydrate(&self, root_id: Uuid, path: String) -> Result<(), DaemonError>;

    /// Keep only paths matching `patterns` resident; stub the rest.
    ///
    /// Stored, so it governs later materializes too — content arriving
    /// from a peer lands resident or as a stub according to it. Empty
    /// patterns clear the policy, which means "keep everything".
    async fn keep_only(
        &self,
        root_id: Uuid,
        patterns: Vec<String>,
    ) -> Result<KeptReport, DaemonError>;

    /// What this root keeps resident. Empty means everything.
    async fn kept(&self, root_id: Uuid) -> Result<Vec<String>, DaemonError>;

    /// The reverse: release one path's resident bytes, leaving the file
    /// listed at its real size as a pointer stub. Opening it — through a
    /// mount, or through `hydrate` — brings it back.
    async fn dehydrate(&self, root_id: Uuid, path: String) -> Result<(), DaemonError>;

    /// Settle one path two machines changed independently, by keeping
    /// every side: the first keeps the name, the others land beside it
    /// as `<stem> (divergent n).<ext>`.
    ///
    /// Keeping both rather than picking is the only honest choice from
    /// here — a person at a terminal has been told two machines disagree
    /// about a file and can see neither version. Picking one would be
    /// choosing which work to discard on the strength of a path name;
    /// this puts both on the disk, where the file can be opened and the
    /// real decision made.
    async fn keep_both(&self, root_id: Uuid, path: String) -> Result<(), DaemonError>;

    /// Checkpoint one synced root's live tree now — the same explicit
    /// Session checkpoint the RPC surface exposes, driven locally so a
    /// user can force a save point before unplugging.
    async fn checkpoint_now(&self, root_id: Uuid) -> Result<(), DaemonError>;

    /// Mount a root's live tree at `mountpoint`, so the whole tree is
    /// browsable and anything this machine does not hold is fetched by
    /// the act of opening it — the cloud-folder behaviour.
    ///
    /// Linux only from the agent: macOS reaches the same place through a
    /// File Provider extension, which the system loads from the app
    /// bundle and which is itself a client of this surface.
    async fn mount(&self, root_id: Uuid, mountpoint: String) -> Result<(), DaemonError>;

    /// Take the mount down. The tree stays where it is on disk — the
    /// mount was a window onto it, not the thing itself.
    async fn unmount(&self, root_id: Uuid) -> Result<(), DaemonError>;

    /// Every root mounted right now, and where.
    async fn mounts(&self) -> Result<Vec<(Uuid, String)>, DaemonError>;

    /// Say where a root appears in the tree people are shown — a
    /// relative path like `codywright/Projects/Some Record`.
    ///
    /// Unrelated to where the bytes are, on purpose. A disk is laid out
    /// by fifteen years of accidents; the tree somebody should see is
    /// not that, and reshaping terabytes to make the two agree would be
    /// an expensive answer to a question about presentation.
    async fn set_place(&self, root_id: Uuid, place: String) -> Result<(), DaemonError>;

    /// Mount every root this machine holds, composed into one tree
    /// under `under` by their places. Returns each place and what went
    /// wrong with it, if anything.
    /// `flat` drops the org from each place, so every project lands in
    /// one `Projects/` and every asset in one `Assets/`. The org is how
    /// the work is stored — whose it is, who is billed — not how anybody
    /// looks for it, and grouping by it turns "find that March session"
    /// into a search across six folders instead of a glance at one.
    async fn mount_all(
        &self,
        under: String,
        flat: bool,
    ) -> Result<Vec<(String, Option<String>)>, DaemonError>;

    /// Live status changes as they happen.
    #[subscribe]
    fn status_events(&self) -> DaemonEvent;
}
