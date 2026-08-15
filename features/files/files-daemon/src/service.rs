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
