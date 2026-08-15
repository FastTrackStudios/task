//! Live run state — one subscribe stream plus a snapshot to join on.
//!
//! Subscribers **fetch once, then fold**: take [`RunStream::snapshot`]
//! for the run you care about, then apply every
//! [`RunEventEnvelope`] whose `run` matches. That is why a viewer
//! arriving mid-run sees recent output instead of an empty pane.

use uuid::Uuid;

use crate::error::AgentError;
use crate::run_event::{RunEvent, RunEventEnvelope, RunSnapshot};

#[architect::rpc]
pub trait RunStream {
    /// Every run event, as it happens. Filter by `run` client-side.
    #[subscribe]
    fn run_events(&self) -> RunEventEnvelope;

    /// The state to fold onto — status, current activity, and the
    /// bounded output tail.
    async fn snapshot(&self, run: Uuid) -> Result<RunSnapshot, AgentError>;

    /// Publish an event. Called by the runner that owns the run.
    ///
    /// Runs have exactly one writer, which is what lets a plain
    /// stream stand in for a merged document.
    async fn publish(&self, run: Uuid, event: RunEvent) -> Result<(), AgentError>;
}
