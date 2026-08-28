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

    /// Share a folder from this machine: version it, checkpoint it, and
    /// serve it to admitted peers. Returns the root's id and name.
    ///
    /// Without this the agent could only ever hold what somebody else
    /// already held — fine for a laptop taking an org's projects, and
    /// useless for two machines that just want the same folder.
    async fn share(&self, path: String, name: Option<String>)
    -> Result<(Uuid, String), DaemonError>;

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

    /// Checkpoint one synced root's live tree now — the same explicit
    /// Session checkpoint the RPC surface exposes, driven locally so a
    /// user can force a save point before unplugging.
    async fn checkpoint_now(&self, root_id: Uuid) -> Result<(), DaemonError>;

    /// Live status changes as they happen.
    #[subscribe]
    fn status_events(&self) -> DaemonEvent;
}
