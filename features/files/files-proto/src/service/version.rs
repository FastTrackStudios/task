//! History, divergence and restore — `files.version.*`,
//! `files.concurrency.*`.
//!
//! The versioned unit is what the application opens, which is often not
//! one file: a Reaper `.rpp` is a single text file, a Logic project a
//! directory, a Pro Tools session a folder, a Resolve project a database
//! row that becomes a file only on export. Callers address the unit; the
//! shape of the format is our problem.
//!
//! Concurrency lives here rather than in a lane of its own because the
//! advisory signal and the divergence it fails to prevent are two ends of
//! one mechanism: nothing blocks, and both sides survive.

use chrono::{DateTime, Utc};
use facet::Facet;

use crate::error::FilesFault;
use crate::id::{PrincipalId, RootId, SnapshotId, VersionId};
use crate::model::{ChainEntry, CheckpointInfo, DivergenceInfo, SnapshotInfo};
use crate::path::RootPath;

/// Someone has an unmergeable file open.
///
/// Advisory only: it gates no RPC, rejects no write, needs no release,
/// and expires on its own when a client vanishes. A lock that strands a
/// file when its holder closes a laptop is worse than the collision it
/// prevents.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Occupancy {
    pub root_id: RootId,
    pub path: RootPath,
    pub principal: PrincipalId,
    pub display_name: String,
    pub since: DateTime<Utc>,
    /// Lapses on its own without a heartbeat. Nothing needs to release
    /// it, because nothing was held.
    pub expires_at: DateTime<Utc>,
}

/// How a divergence was settled. Doing nothing keeps both, which is the
/// point — the safe outcome requires no action.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(u8)]
pub enum Resolution {
    KeepMine,
    KeepTheirs,
    /// Keep both, each under its own name.
    KeepBoth {
        mine: String,
        theirs: String,
    },
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(u8)]
pub enum VersionEvent {
    /// A session ended in a certified checkpoint.
    Checkpointed(CheckpointInfo),
    /// The cadence engine took an ephemeral auto-snapshot during
    /// activity. Not a version.
    Snapshotted(SnapshotInfo),
    /// Concurrent edits produced two versions. Both survive.
    Diverged(DivergenceInfo),
    DivergenceResolved(DivergenceInfo),
    /// Someone opened, or stopped holding, an unmergeable file.
    OccupancyChanged(Occupancy),
    Restored(ChainEntry),
}

#[architect::rpc]
pub trait VersionService {
    /// A path's version chain, newest first, following recorded renames.
    /// Empty when it has never been checkpointed.
    async fn chain(&self, root_id: RootId, path: RootPath) -> Result<Vec<ChainEntry>, FilesFault>;

    /// Certify a checkpoint now: scan the live tree, diff against head,
    /// write one operation.
    ///
    /// The explicit half of Session checkpoint; the other half is
    /// per-root quiescence, driven by the cadence engine. Never per
    /// write — that is thousands of operations a day.
    async fn checkpoint(
        &self,
        root_id: RootId,
        description: Option<String>,
    ) -> Result<CheckpointInfo, FilesFault>;

    /// Ephemeral snapshots taken during activity, newest first.
    async fn snapshots(
        &self,
        root_id: RootId,
        limit: Option<u32>,
    ) -> Result<Vec<SnapshotInfo>, FilesFault>;

    /// Signal that this principal has an unmergeable file open, and keep
    /// the signal alive. Idempotent; call it on a timer.
    async fn hold(&self, root_id: RootId, path: RootPath) -> Result<Occupancy, FilesFault>;

    /// Who else has this open. Read before an edit begins — the whole
    /// point is being told before rather than after.
    async fn occupancy(
        &self,
        root_id: RootId,
        path: RootPath,
    ) -> Result<Vec<Occupancy>, FilesFault>;

    /// Unresolved divergences in this root.
    async fn divergences(&self, root_id: RootId) -> Result<Vec<DivergenceInfo>, FilesFault>;

    /// Settle one. Never automatic: a merge of a format we do not
    /// understand yields a file the application refuses to open.
    async fn resolve_divergence(
        &self,
        root_id: RootId,
        version: VersionId,
        resolution: Resolution,
    ) -> Result<DivergenceInfo, FilesFault>;

    /// Restore a past version of a path.
    ///
    /// Non-destructive: produces a new version and discards nothing.
    /// Media references resolve through recorded rename history and
    /// content addresses rather than stale paths, so a year-old session
    /// opens rather than arriving full of missing media.
    async fn restore(
        &self,
        root_id: RootId,
        path: RootPath,
        version: VersionId,
    ) -> Result<ChainEntry, FilesFault>;

    /// Promote an ephemeral snapshot to a durable version, for the case
    /// where the interesting state was mid-session.
    async fn keep_snapshot(
        &self,
        root_id: RootId,
        snapshot: SnapshotId,
    ) -> Result<CheckpointInfo, FilesFault>;
}
