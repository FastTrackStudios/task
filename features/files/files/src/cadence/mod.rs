//! The **cadence engine** (issue #260): the machinery that makes the
//! Session-checkpoint guarantee true without anyone ever pressing save
//! — "everything is versioned by the end of a working session"
//! (glossary, `apps/task/CONTEXT.md`).
//!
//! Four parts:
//!
//! - [`clock`] — the injected clock. Quiescence is 30 minutes; no test
//!   waits that out, so the engine never reads the wall clock directly.
//! - [`engine`] — the state machine. Activity in, *what is due now* out.
//! - [`journal`] — the durable per-root record: which head is the
//!   checkpoint head, and the save points/snapshots that are metadata
//!   rather than commit content.
//! - [`watcher`] — the server-side filesystem watcher, which produces
//!   *hints* and nothing more.
//!
//! # Hints versus truth
//!
//! The watcher is not authoritative and cannot be: inotify is blind on
//! NFS clients, and Files' whole point is a NAS full of DAW sessions
//! (spec #255 — "change detection is authoritative on the storage
//! server", and even there a watcher can drop events under load). So a
//! hint only ever answers *is this root busy, and when did it last look
//! busy* — the timing question the cadence is made of. What actually
//! goes into a version is decided by a full stat-scan of the live tree
//! at capture time, certified file by file (see [`crate::certify`]).
//! A root that got no hints at all still checkpoints correctly the
//! moment anything asks it to; a root that got spurious hints
//! checkpoints an unchanged tree, which is a no-op capture.
//!
//! # Snapshots are not versions
//!
//! An auto-snapshot is "an ephemeral safety capture ... never a chain
//! entry" (glossary). That is structural here, not a filter applied
//! after the fact: snapshot commits are parented on the previous
//! *snapshot* (or on the checkpoint head, for the session's first),
//! forming a side branch, while the next checkpoint parents on the
//! checkpoint head itself. `chain::version_chain` walks parents from
//! the checkpoint head, so it walks the checkpoint line and never sees
//! a snapshot — no description convention to parse, no second index to
//! keep honest, and the snapshots stay fully reachable for recovery
//! until retention expires them.

pub mod clock;
pub mod engine;
pub mod journal;
pub mod watcher;

pub use clock::{Clock, SystemClock, TestClock};
pub use engine::{CadenceConfig, CadenceEngine, Due, DueKind};
pub use journal::Journal;
pub use watcher::{ActivitySink, RootWatcher};
