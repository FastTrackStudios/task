//! The per-root cadence journal: `<root>/.fts-files/cadence.json`.
//!
//! Two jobs, both of which have to survive a server restart:
//!
//! 1. **Which head is the checkpoint head.** Auto-snapshot commits
//!    branch off the checkpoint line rather than sitting on it (that is
//!    what keeps them out of every version chain — see
//!    [`crate::cadence`]'s module doc), so a root's jj view legitimately
//!    carries more than one head and "pick whichever head the view
//!    lists first" would sometimes pick a snapshot. The journal records
//!    the real checkpoint head, so reopening a root resumes the
//!    checkpoint line rather than a snapshot branch.
//! 2. **Save points and snapshot metadata.** Save points are display
//!    metadata, not versions (glossary), so there is nowhere in the
//!    commit graph they belong; this is where they live, keyed by the
//!    capture that carries them. It is what makes them visible on a
//!    chain entry and on [`files_proto::SnapshotInfo`].
//!
//! The journal is metadata *about* the store, never a second authority
//! over its contents: lose it and every checkpoint and snapshot is
//! still in the repo, still reachable, just missing its labels.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use files_proto::{SavePoint, SnapshotInfo};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;

/// Filename of the journal inside a root's store dir.
pub const JOURNAL_FILE: &str = "cadence.json";

/// How long an auto-snapshot stays listed (spec #255: "expire after 14
/// days"). Pruning the record here stops an expired snapshot being
/// offered for recovery; reclaiming its objects is GC's half of
/// retention (`task_files_version_store::gc`, issue #258).
pub const SNAPSHOT_RETENTION: TimeDelta = TimeDelta::days(14);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRecord {
    /// Hex-encoded jj `CommitId` of the snapshot commit.
    pub snapshot_id: String,
    pub at: DateTime<Utc>,
    pub changed_paths: Vec<String>,
    pub save_points: Vec<SavePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRecord {
    /// Hex-encoded jj `CommitId` of the checkpoint commit.
    pub commit_id: String,
    pub at: DateTime<Utc>,
    pub save_points: Vec<SavePoint>,
    pub requeued_paths: Vec<String>,
}

/// One root's durable cadence state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Journal {
    /// Newest certified checkpoint — the head every chain walk starts
    /// from and every new checkpoint parents onto.
    pub checkpoint_head: Option<String>,
    /// Newest auto-snapshot on the branch hanging off `checkpoint_head`,
    /// or `None` when no snapshot has been taken since it.
    pub snapshot_head: Option<String>,
    /// Auto-snapshots, oldest first, pruned at [`SNAPSHOT_RETENTION`].
    pub snapshots: Vec<SnapshotRecord>,
    /// Certified checkpoints, oldest first.
    pub checkpoints: Vec<CheckpointRecord>,
}

impl Journal {
    fn path(store_dir: &Path) -> PathBuf {
        store_dir.join(JOURNAL_FILE)
    }

    /// Read the journal for the root whose store dir is `store_dir`;
    /// a root that has never checkpointed simply has none yet.
    ///
    /// **An unreadable journal is an empty journal, never an error.**
    /// This file holds labels, not content (see the module doc), so a
    /// truncated or hand-mangled `cadence.json` must cost a root its
    /// save points and its snapshot listing — not its ability to
    /// checkpoint, browse, or derive a chain. Hard-failing here would
    /// wedge every one of those RPCs permanently until somebody deleted
    /// the file by hand (PR #283 review). The heads degrade to the
    /// repo's own view heads, and the next capture writes a fresh,
    /// valid journal.
    pub fn load(store_dir: &Path) -> Result<Self> {
        let path = Self::path(store_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "files: cadence journal unreadable, continuing without its labels");
                return Ok(Self::default());
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(journal) => Ok(journal),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "files: cadence journal corrupt, continuing without its labels");
                Ok(Self::default())
            }
        }
    }

    /// Write the journal, atomically: a temp file in the same directory
    /// (so `rename` stays within one filesystem) replaced over the real
    /// one. A crash or ENOSPC then leaves either the old journal or the
    /// new one, never a half-written file — and a reader that takes no
    /// lock (`chain` derives save points concurrently with a capture)
    /// always observes one complete version.
    pub fn save(&self, store_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(store_dir)?;
        let path = Self::path(store_dir);
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temp, &path)?;
        Ok(())
    }

    /// Record an auto-snapshot: it becomes the snapshot head, and any
    /// snapshot older than [`SNAPSHOT_RETENTION`] stops being listed.
    ///
    /// `checkpoint_head` is the commit the snapshot branch hangs off —
    /// recorded here too, and not only by [`Journal::record_checkpoint`],
    /// because a session's *first* snapshot is exactly the moment the
    /// view stops having one obvious head. Without it, a root that has
    /// snapshotted but never checkpointed would fall back to "the first
    /// view head" and pick a snapshot as its checkpoint line, putting
    /// every ephemeral capture into the version chain.
    pub fn record_snapshot(
        &mut self,
        record: SnapshotRecord,
        checkpoint_head: &str,
        now: DateTime<Utc>,
    ) {
        self.checkpoint_head = Some(checkpoint_head.to_string());
        self.snapshot_head = Some(record.snapshot_id.clone());
        self.snapshots.push(record);
        self.snapshots.retain(|s| now - s.at < SNAPSHOT_RETENTION);
    }

    /// Record a certified checkpoint: it becomes the checkpoint head,
    /// and the snapshot branch it supersedes is closed out (the
    /// snapshots stay listed until they expire — a mistake made during
    /// the session is still recoverable after it ends).
    pub fn record_checkpoint(&mut self, record: CheckpointRecord) {
        self.checkpoint_head = Some(record.commit_id.clone());
        self.snapshot_head = None;
        self.checkpoints.push(record);
    }

    /// The save points recorded for the checkpoint `commit_id` — how a
    /// chain entry gets its save-point metadata.
    #[must_use]
    pub fn save_points_for(&self, commit_id: &str) -> Vec<SavePoint> {
        self.checkpoints
            .iter()
            .find(|c| c.commit_id == commit_id)
            .map(|c| c.save_points.clone())
            .unwrap_or_default()
    }

    /// Listed snapshots as wire types, newest first.
    #[must_use]
    pub fn snapshot_infos(&self, root_id: Uuid) -> Vec<SnapshotInfo> {
        self.snapshots
            .iter()
            .rev()
            .map(|s| SnapshotInfo {
                root_id,
                snapshot_id: s.snapshot_id.clone(),
                at: s.at,
                changed_paths: s.changed_paths.clone(),
                save_points: s.save_points.clone(),
            })
            .collect()
    }
}
