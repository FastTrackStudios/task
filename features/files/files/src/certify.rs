//! Certification: reading a live-tree file into the CAS *and proving
//! the read was of one coherent state*.
//!
//! Spec #255 / issue #260: "Checkpoint certification runs a full
//! stat-scan; a file changing mid-hash is requeued, not corrupted."
//! Files here are DAW sessions and multi-GB media being written by
//! another process at the same moment we hash them — a 40 GB render is
//! minutes of streaming, and a torn read of it would be committed as a
//! perfectly valid-looking version of a file that never existed.
//!
//! The guard is a stat sandwich: `stat` the file, stream it into the
//! chunk store, `stat` it again. If anything moved, the bytes we hashed
//! were a moving target — retry, and after
//! [`CadenceConfig::certify_attempts`](crate::cadence::CadenceConfig::certify_attempts)
//! attempts give up on *this* file only. Giving up means the file keeps
//! whatever state it already had in the store and rides into the next
//! capture; the capture in progress still succeeds for everything else.
//! A writer that never pauses would otherwise be able to block a whole
//! root's history indefinitely.
//!
//! # What "anything moved" has to mean on a NAS
//!
//! Length plus mtime is not enough, and this deployment is precisely
//! where that bites (PR #283 review): several DAW and media writers
//! rewrite blocks *in place*, so the length never changes, and mtime
//! granularity on FAT/exFAT, some NAS appliances, and NFSv3-visible
//! attributes is one to two seconds. A same-length in-place rewrite
//! landing inside the pre-stat's granule would leave `before == after`
//! and certify a torn read — exactly what this module exists to stop.
//!
//! So [`FileStat`] carries every cheap identity signal the platform
//! offers — length, mtime, and on unix the inode and ctime (ns-granular
//! in the kernel even where a filesystem's mtime is displayed coarse) —
//! and when *none* of them can prove sub-second resolution
//! ([`FileStat::is_coarse`]), certification stops trusting timestamps
//! and re-reads the file: two independent streaming passes that hash to
//! the same content address did not have a write between them. That
//! costs a second read only on the filesystems that cannot prove
//! otherwise. A file whose metadata cannot be read at all is requeued
//! rather than certified on length alone.
//!
//! An abandoned attempt does leave its chunks (and a manifest) behind in
//! the CAS. That is exactly what `ChunkStore::gc`'s manifest sweep is
//! for: nothing in any commit tree references the abandoned `FileId`, so
//! the next GC pass reclaims it (issue #258).

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;

/// Test seam: a callback invoked after the pre-read `stat` and before
/// the streaming read, so a test can make a file change *during* its
/// own hash deterministically instead of racing a background writer.
/// Production never sets one.
pub type MidHashHook = Arc<dyn Fn(&Path) + Send + Sync>;

/// The identity a stat sandwich compares. Deliberately not a content
/// hash — the point is to detect that the file moved under us without
/// reading it a second time (and when this evidence is too coarse to
/// settle it, [`stream_certified`] does read it a second time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStat {
    len: u64,
    /// `None` only when the platform cannot report it — which is itself
    /// disqualifying (see [`FileStat::is_coarse`]).
    modified: Option<SystemTime>,
    /// Inode, and ctime as (seconds, nanoseconds). Unix only; a rename-
    /// over-the-top write changes the inode without necessarily moving
    /// length or mtime, and ctime moves on every write.
    #[cfg(unix)]
    unix: Option<(u64, i64, i64)>,
}

/// Sub-second components of `time`, or `None` when it predates the
/// epoch (in which case we simply have no resolution evidence).
fn subsec_nanos(time: SystemTime) -> Option<u32> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.subsec_nanos())
}

impl FileStat {
    fn read(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            unix: {
                use std::os::unix::fs::MetadataExt as _;
                Some((metadata.ino(), metadata.ctime(), metadata.ctime_nsec()))
            },
        })
    }

    /// Can this stat pair rule out a same-length in-place rewrite on its
    /// own? It can only do so if *some* timestamp it carries has
    /// sub-second resolution — a non-zero nanosecond component is proof
    /// that the filesystem records finer than a granule. All-zero
    /// nanoseconds means either a coarse filesystem or a
    /// one-in-a-billion coincidence, and both are handled the same way:
    /// verify by re-reading, which is correct in either case and rare in
    /// the second.
    pub(crate) fn is_coarse(&self) -> bool {
        #[cfg(unix)]
        if let Some((_, _, ctime_nsec)) = self.unix {
            if ctime_nsec != 0 {
                return false;
            }
        }
        match self.modified.and_then(subsec_nanos) {
            Some(nanos) => nanos == 0,
            // No usable mtime at all: never certify on length alone.
            None => true,
        }
    }
}

/// What a stat sandwich concluded about one read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// The file provably held still: commit what was read.
    Stable,
    /// The stats matched, but nothing in them has the resolution to
    /// rule out a same-length in-place rewrite (see the module doc).
    /// The caller must confirm by content — re-read and compare ids.
    Coarse,
    /// The file moved under the read: retry, or requeue it.
    Moved,
}

/// The "before" half of a stat sandwich, taken before a file is read
/// into the store and consulted again after.
///
/// Flavor-agnostic on purpose (issue #273 generalized the checkpoint
/// writer to jj's `Backend` trait): this measures the *file*, so it
/// guards a media root's CAS write and a software root's git blob write
/// identically.
#[derive(Debug)]
pub struct StatGuard {
    before: FileStat,
}

impl StatGuard {
    /// Stat `path` before reading it.
    pub fn begin(path: &Path) -> Result<Self> {
        Ok(Self {
            before: FileStat::read(path)?,
        })
    }

    /// Stat `path` again and judge the read.
    pub fn check(&self, path: &Path) -> Result<Settled> {
        let after = FileStat::read(path)?;
        if after != self.before {
            return Ok(Settled::Moved);
        }
        Ok(if after.is_coarse() {
            Settled::Coarse
        } else {
            Settled::Stable
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn stat(modified: Option<SystemTime>, ctime_nsec: i64) -> FileStat {
        FileStat {
            len: 42,
            modified,
            #[cfg(unix)]
            unix: Some((7, 1_700_000_000, ctime_nsec)),
        }
    }

    #[test]
    fn sub_second_resolution_settles_certification_on_stats_alone() {
        let ns = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789);
        assert!(!stat(Some(ns), 0).is_coarse());
    }

    #[cfg(unix)]
    #[test]
    fn a_nanosecond_ctime_is_enough_even_when_mtime_reads_whole_seconds() {
        let coarse = UNIX_EPOCH + Duration::new(1_700_000_000, 0);
        assert!(!stat(Some(coarse), 456).is_coarse());
    }

    #[test]
    fn whole_second_timestamps_force_a_content_re_read() {
        let coarse = UNIX_EPOCH + Duration::new(1_700_000_000, 0);
        assert!(
            stat(Some(coarse), 0).is_coarse(),
            "a 1-2s mtime granule cannot rule out a same-length in-place rewrite"
        );
    }

    #[test]
    fn an_unreadable_mtime_never_certifies_on_length_alone() {
        assert!(stat(None, 0).is_coarse());
    }
}
