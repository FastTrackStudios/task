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
//! - [`filter`] — the two questions the engine asks about a path.
//! - [`journal`] — the durable per-root record: which head is the
//!   checkpoint head, and the save points/snapshots that are metadata
//!   rather than commit content.
//!
//! The filesystem watcher is deliberately **not** here. It is an
//! inotify/FSEvents adapter, and its own doc says it produces *hints and
//! nothing more* — an OS integration rather than a rule, so it stays in
//! `files` alongside the other adapters.
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
//! at capture time, certified file by file (`files::certify`).
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
pub mod filter;
pub mod journal;

pub use clock::{Clock, SystemClock, TestClock};
pub use engine::{CadenceConfig, CadenceEngine, Due, DueKind};
pub use filter::{ActivityFilter, PassThrough, SuffixFilter};
pub use journal::Journal;

/// What can go wrong keeping the journal.
///
/// The journal is the only part of the cadence that touches a disk, and
/// it does so for its own file — not for a root's tree, not for a version
/// store. So it carries a two-variant error of its own rather than the
/// `files` crate's, which would drag jj-lib and the version store into a
/// crate that needs neither.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("journal json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
