//! Run records — the server's memory of every attempt.
//!
//! Runners report; the server remembers. Nothing here reaches into a
//! machine: a runner says "I started this, here is where the worktree
//! is", and later "it finished, here is the exit code".

use crate::error::AgentError;
use crate::run::{FinishRun, Run, RunFilter, StartRun};

#[architect::rpc]
pub trait Runs {
    /// Record the start of an attempt. Returns the created run.
    async fn start_run(&self, start: StartRun) -> Result<Run, AgentError>;

    /// Say an in-progress attempt is still going.
    async fn beat_run(&self, run_id: uuid::Uuid) -> Result<(), AgentError>;

    /// Record the end of an attempt.
    async fn finish_run(&self, finish: FinishRun) -> Result<Run, AgentError>;

    /// One run.
    async fn get_run(&self, run_id: uuid::Uuid) -> Result<Run, AgentError>;

    /// Runs matching a filter, newest first.
    async fn list_runs(&self, filter: RunFilter) -> Result<Vec<Run>, AgentError>;

    /// Mark a run's worktree as reclaimed.
    ///
    /// Moves `needs-cleanup` to `archived`; the session file is kept
    /// so a resumable attempt stays resumable.
    async fn archive_run(&self, run_id: uuid::Uuid) -> Result<Run, AgentError>;

    /// Move in-progress runs whose heartbeat has lapsed to `stale`,
    /// returning how many changed.
    ///
    /// Deliberately a call rather than a background sweep: the server
    /// has no timer of its own for this, and a caller that wants
    /// fresh liveness can ask for it.
    async fn sweep_stale_runs(&self) -> Result<u32, AgentError>;
}
