//! The control surface's wire model (issue #265). The centrepiece is
//! [`DaemonStatus`]: not a bare "syncing / synced" but every synced
//! root's state with **per-file progress** — which file is moving,
//! how many of its chunks are local, how many bytes — so the desktop
//! app and CLI can show real information about a sync in flight.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a synced root stands right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum RootSyncState {
    /// Chosen for sync, nothing in flight — fully reconciled.
    #[default]
    Idle,
    /// A reconcile pull is running.
    Syncing,
    /// Paused by the user; no pulls until resumed.
    Paused,
    /// The last pull failed (see [`RootStatus::last_error`]); the
    /// daemon retries on its next tick.
    Error,
}

/// One file's live transfer within a running pull — the granular
/// information the user asked to see. `chunks_done` of `chunks_total`
/// are local, carrying `bytes_done` of `logical_bytes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct FileProgress {
    /// Root-relative path.
    pub path: String,
    pub chunks_done: u32,
    pub chunks_total: u32,
    pub bytes_done: u64,
    pub logical_bytes: u64,
    /// `true` once the file's content is fully local.
    pub done: bool,
}

impl FileProgress {
    /// 0..=100, saturating — the number a progress bar shows.
    #[must_use]
    pub fn percent(&self) -> u8 {
        if self.chunks_total == 0 {
            return 100;
        }
        ((self.chunks_done as u64 * 100) / self.chunks_total as u64).min(100) as u8
    }
}

/// One synced root's full status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct RootStatus {
    pub root_id: Uuid,
    pub name: String,
    pub state: RootSyncState,
    /// The selective-sync slice, empty for "the whole root" (gitignore
    /// dialect, the hydration-policy patterns — a partial replica keeps
    /// matching paths resident, the rest as stubs).
    pub slice: Vec<String>,
    /// Files currently mid-transfer, most-recently-touched first. Empty
    /// when [`RootSyncState::Idle`].
    pub files: Vec<FileProgress>,
    /// Cumulative counters for the pull in flight (or the last one).
    pub chunks_fetched: u64,
    pub chunks_skipped: u64,
    /// When the last pull completed, if any.
    pub last_synced_at: Option<DateTime<Utc>>,
    /// The last pull's error, if it failed.
    pub last_error: Option<String>,
}

impl RootStatus {
    /// Aggregate percent across the files in flight — the single number
    /// a compact UI shows for the whole root.
    #[must_use]
    pub fn percent(&self) -> u8 {
        let (done, total): (u64, u64) = self
            .files
            .iter()
            .map(|f| (u64::from(f.chunks_done), u64::from(f.chunks_total)))
            .fold((0, 0), |(d, t), (fd, ft)| (d + fd, t + ft));
        if total == 0 {
            return 100;
        }
        ((done * 100) / total).min(100) as u8
    }
}

/// The daemon's whole state — the reply to
/// [`crate::service::DaemonControlService::status`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct DaemonStatus {
    /// This machine's device (agent) id, once enrolled.
    pub device_id: Option<Uuid>,
    /// Whether the operator has approved this device.
    pub enrolled: bool,
    /// Global pause — no root syncs while set.
    pub paused: bool,
    /// Every root the daemon is set to sync.
    pub roots: Vec<RootStatus>,
}
