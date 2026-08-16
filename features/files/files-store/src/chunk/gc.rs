//! Chunk-level garbage collection primitive (issue #258).
//!
//! iroh-blobs 0.103's only *supported* deletion path is its own periodic
//! mark-and-sweep task: `Blobs::delete` is `pub(crate)` ("Users should rely
//! only on garbage collection for blob deletion" — that method's own doc),
//! and `store::gc::gc_run_once` isn't reachable from outside the crate
//! either (`mod gc;`, not `pub mod gc;`, in `iroh_blobs::store`). The one
//! public surface is [`iroh_blobs::store::fs::Options::gc`]: a
//! [`iroh_blobs::store::GcConfig`] set at store-open time, which runs a
//! background task on a fixed interval and consults an `add_protected`
//! callback before each sweep.
//!
//! **The callback derives liveness live, from the manifests directory,
//! every time it runs — it never reads a snapshot [`crate::chunk::ChunkStore::gc`]
//! published.** An earlier version of this file had `gc` publish a
//! computed chunk-hash set into shared state for the callback to read; that
//! left a real data-loss window open, since iroh-blobs' background task
//! fires on *its own* interval, independent of `gc` calls: a chunk written
//! after the last `gc` call (and therefore absent from that stale snapshot)
//! would be swept by the very next background tick, before anything ever
//! called `gc` again. Deriving straight from what's on disk closes that
//! window unconditionally — a chunk is protected as long as some manifest
//! currently on disk references it, full stop, with no dependency on `gc`
//! having run recently. `gc`'s own job shrinks to exactly one thing:
//! synchronously deciding which manifests to keep (`protected`/`keep_newer`)
//! and durably removing the rest — once a manifest is gone, the next
//! callback invocation simply won't see it, and its now-unreferenced chunks
//! become eligible for reclamation.
//!
//! If *any* manifest fails to read or decode during the callback's scan,
//! the whole pass aborts (`ProtectOutcome::Abort`) rather than just
//! skipping that one file. This has to be conservative: a permanently
//! corrupt manifest and one that's perfectly fine but hit a transient I/O
//! hiccup on this particular scan are indistinguishable from here, and
//! treating "failed to read" as "nothing to protect for it" is exactly how
//! a healthy manifest's chunk gets swept out from under it — irreversibly,
//! since deleting a chunk can't be undone by the *next* pass reading it
//! correctly. (An earlier version of this callback skipped just the
//! failing manifest; that is what let a handful of chunks vanish under
//! sustained write/sweep pressure in testing even though every manifest on
//! disk was intact — the file that failed to read on one particular scan
//! wasn't corrupt, just transiently unlucky, and skipping it cost another,
//! unrelated chunk its only protection for that pass.) The cost is a store
//! with one permanently unreadable *and* permanently kept (protected or
//! never aging past `keep_newer`) manifest wedging reclamation store-wide
//! until that manifest is dealt with — `gc`'s own manifest *removal*
//! decision is unaffected (it never decodes a kept manifest's contents at
//! all, only its `FileId`/mtime), so an expired corrupt manifest still gets
//! swept and stops blocking future passes on its own.

use std::time::Duration;

/// The default chunk-level GC interval, shared with
/// `task-files-version-store`'s `VersionStoreBackend::DEFAULT_GC_INTERVAL`
/// so the two layers' production cadence can't silently diverge.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Chunk-level GC configuration for [`crate::chunk::ChunkStore::open_with_gc`]:
/// how often iroh-blobs' own background sweep is allowed to run.
#[derive(Debug, Clone, Copy)]
pub struct GcConfig {
    pub interval: Duration,
}

impl Default for GcConfig {
    /// A conservative production default — chunk reclamation is not
    /// latency-sensitive (unlike manifest removal, which `ChunkStore::gc`
    /// performs synchronously). Tests that need to observe a sweep pass
    /// within their own runtime should pass a much shorter interval
    /// explicitly.
    fn default() -> Self {
        Self {
            interval: DEFAULT_INTERVAL,
        }
    }
}

/// The outcome of one [`crate::chunk::ChunkStore::gc`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcStats {
    /// Manifests removed this call (unreferenced by the caller's
    /// `protected` set and older than `keep_newer`).
    pub manifests_swept: usize,
}
