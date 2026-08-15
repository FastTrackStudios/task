//! Events emitted by the sync engine. Superset of
//! [`email_proto::EmailEvent`] — we add per-cycle progress
//! signals (`CycleStarted` / `CycleCompleted`) so the UI can
//! render "syncing…" spinners + "last synced X ago" labels
//! without polling.

use email_proto::EmailEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncEvent {
    /// Sync cycle began. `folder_count` is the previous
    /// snapshot's count, for spinner sizing.
    CycleStarted { folder_count: usize },
    /// Cycle finished. `duration_ms` measured wall-clock.
    CycleCompleted { duration_ms: u64 },
    /// Cycle failed. The engine keeps running.
    CycleFailed { reason: String },
    /// One reconciliation event. Mirrors the proto's
    /// `EmailEvent` 1:1 so subscribers can treat both
    /// channels uniformly.
    Email(EmailEvent),
}
