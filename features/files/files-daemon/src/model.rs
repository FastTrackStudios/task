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
    ///
    /// Bytes first, chunks only as a fallback. The store links a large
    /// file whole (a link costs nothing at any size), so a multi-gigabyte
    /// take is a *one-chunk* manifest pulled as verified windows: counted
    /// in chunks its progress is 0% for the length of the transfer and
    /// 100% at the end, which is the one file where a person actually
    /// needs the bar. Bytes move the whole way down either path.
    #[must_use]
    pub fn percent(&self) -> u8 {
        if self.logical_bytes > 0 {
            return ((self.bytes_done * 100) / self.logical_bytes).min(100) as u8;
        }
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
    /// The endpoint this root is pulled from, when it was chosen against
    /// one. "Where does this come from" is the first question about a
    /// root that is not updating, and without it the answer lived only
    /// in a client the status surface could not describe.
    pub peer: Option<String>,
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
    ///
    /// Weighted by bytes for [`FileProgress::percent`]'s reason, and
    /// because the alternative is worse here than per-file: a root
    /// holding one 80 GB reel beside two hundred small stems would
    /// otherwise read as nearly finished while the only file that costs
    /// anything had not started.
    #[must_use]
    pub fn percent(&self) -> u8 {
        let (done, total): (u64, u64) = self
            .files
            .iter()
            .map(|f| (f.bytes_done, f.logical_bytes))
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
    /// This machine's device id — minted on first run, persisted, and
    /// the same across every restart.
    pub device_id: Option<Uuid>,
    /// This machine's endpoint id: its address on the network and the
    /// string an org admits to let it sync. `None` until the daemon has
    /// bound its endpoint, which is also the state in which nothing can
    /// pull *from* this machine.
    pub endpoint_id: Option<String>,
    /// The endpoints this machine admits to its own replica lane — the
    /// symmetric half of an org admitting the device, and what lets a
    /// server pull this machine's offline work.
    pub peers: Vec<String>,
    /// Whether a coordinator has been dialled: the peer this daemon
    /// pulls from by default.
    pub coordinator: bool,
    /// Global pause — no root syncs while set.
    pub paused: bool,
    /// Every root the daemon is set to sync.
    pub roots: Vec<RootStatus>,
}
