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
    /// The endpoints this root is pulled from.
    ///
    /// A list, not a joined string. It *was* a string, and a caller that
    /// shortened it for display cut the first id out of the middle of
    /// the second — so a root syncing with two machines rendered as one
    /// and looked exactly like the single-peer bug this had just
    /// stopped being. Formatting is the caller's business; giving it one
    /// pre-joined field took that decision away and got it wrong.
    pub peers: Vec<String>,
    pub state: RootSyncState,
    /// The selective-sync slice, empty for "the whole root" (gitignore
    /// dialect, the hydration-policy patterns — a partial replica keeps
    /// matching paths resident, the rest as stubs).
    pub slice: Vec<String>,
    /// Files currently mid-transfer, most-recently-touched first. Empty
    /// when [`RootSyncState::Idle`].
    pub files: Vec<FileProgress>,
    /// Paths that two machines changed independently, awaiting a
    /// person's decision.
    ///
    /// Sync never merges content: concurrent edits survive as sibling
    /// heads and someone picks. Two machines editing the same project is
    /// the *ordinary* case for this product, so a status surface that
    /// cannot say "these two files disagree" leaves the one thing
    /// needing attention invisible — and the tree quietly showing one
    /// side of it.
    pub divergent: Vec<String>,
    /// Cumulative counters for the pull in flight (or the last one).
    pub chunks_fetched: u64,
    pub chunks_skipped: u64,
    /// When the last pull completed, if any.
    pub last_synced_at: Option<DateTime<Utc>>,
    /// The last pull's error, if it failed.
    pub last_error: Option<String>,
    /// Where this root is showing as a folder, if it is.
    ///
    /// Carried in the status rather than fetched separately so a
    /// surface that already polls this — the app's sync page — can show
    /// "mounted at …" without a second round trip that could disagree
    /// with the first.
    pub mounted_at: Option<String>,
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

/// Who made a project, recorded when the folder appeared.
///
/// The OS user rather than a Task account: the agent has no session and
/// no business inventing one. It is enough to attribute the folder and
/// to let the app — which *is* signed in — claim it for the right
/// account the first time somebody looks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct MadeBy {
    /// The OS user, resolved to a name where the machine can, and the
    /// numeric id otherwise.
    pub user: String,
    pub uid: u32,
    /// The machine it was made on — a studio has several, and "who made
    /// this" is half answered by "where".
    pub device: Option<Uuid>,
    pub at: DateTime<Utc>,
}

/// A capture running in the agent, as somebody watching wants it.
///
/// An archive's first capture reads every byte, and on a five-terabyte
/// tree that is hours. Reporting nothing for hours is indistinguishable
/// from being hung — so the agent says which root it is on, how far
/// through the backlog it is, and how big the thing in front of it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct CaptureProgress {
    /// The root being read right now.
    pub root: String,
    /// Its position in the backlog, 1-based, and the backlog's size when
    /// the run began.
    pub done: u32,
    pub total: u32,
    /// What this root weighs, so "no output for twenty minutes" can be
    /// read as "it is a 210 GB session" rather than "it has died".
    pub bytes: u64,
    /// When this root's read began.
    pub since: DateTime<Utc>,
    /// The file being read right now, root-relative. Empty before the
    /// first one — a scan enumerates the tree before it reads anything,
    /// and on a large root that pause is itself worth seeing.
    #[facet(default)]
    pub file: String,
    /// Files read so far, and how many the scan found.
    #[facet(default)]
    pub files_done: u64,
    #[facet(default)]
    pub files_total: u64,
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
    /// Where this agent lands roots it adopts from a peer.
    ///
    /// Reported rather than assumed, because a client that guesses gets
    /// it wrong: the agent's default and the CLI's default were the same
    /// string in two places, the install wrote a different one into the
    /// service unit, and every adoption was then refused as "outside the
    /// permitted boundary" — with the failure swallowed into an empty
    /// list, so the CLI cheerfully reported that the other machine was
    /// sharing nothing.
    pub roots_dir: String,
    /// The capture running right now, if one is. Cleared when the
    /// backlog empties.
    #[facet(default)]
    pub capturing: Option<CaptureProgress>,
    /// Roots registered but never read — they are browsable and would
    /// sync as empty trees. Worth surfacing: it is the one state where
    /// a root looks fine and cannot serve its content.
    #[facet(default)]
    pub awaiting_capture: u32,
    /// Global pause — no root syncs while set.
    pub paused: bool,
    /// Every root the daemon is set to sync.
    pub roots: Vec<RootStatus>,
}
