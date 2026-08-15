//! Slim domain trait for atomic / lifecycle-aware ops over the
//! agent-task queue. Plain CRUD is handled by the architect-
//! emitted `QueueRepo`, `AgentTaskRepo`, `AgentTaskCommentRepo`,
//! `AgentTaskLinkRepo` traits — see `tasks.rs` for the entities.
//!
//! These methods exist because plain CRUD can't enforce them:
//! - `claim_agent_task` is the only sanctioned write that flips
//!   status to `running`; CRUD's `update` is too coarse.
//! - `set_agent_task_status` rejects `running` (must claim).
//! - `complete_agent_task` enforces the "no unresolved blocks
//!   dependency" rule before transitioning to `done`.
//! - `read_queue` assembles a snapshot of one queue's agent
//!   tasks + the latest event-log watermark in one round trip,
//!   so subscribers can resume the tail without an extra call.
//!
//! Parameters are owned (`String`, not `&str`) so the
//! architect-rpc macro can emit a clean async vox dispatcher
//! without leaking borrowed-data lifetimes through the
//! generated async-trait shim.

use crate::error::AgentError;
use crate::tasks::{AgentTask, AgentTaskLink, QueueFilter, QueueSnapshot};

#[architect::rpc]
pub trait AgentTaskQueue {
    async fn read_queue(
        &self,
        queue_id: String,
        filter: QueueFilter,
    ) -> Result<QueueSnapshot, AgentError>;

    /// Atomic claim. Flips status to `running` if the task is
    /// in `ready` AND unclaimed; returns `AgentError::Conflict`
    /// otherwise. The only sanctioned path into `running`.
    async fn claim_agent_task(
        &self,
        agent_task_id: String,
        handle: String,
    ) -> Result<AgentTask, AgentError>;

    /// Set a non-`running` status. Returns `AgentError::Invalid`
    /// if `new_status == "running"` (use `claim_agent_task`)
    /// or if the transition is blocked (e.g. moving to `done`
    /// with unresolved `blocks` dependencies).
    async fn set_agent_task_status(
        &self,
        agent_task_id: String,
        new_status: String,
    ) -> Result<AgentTask, AgentError>;

    /// Mark an agent task `done` with a result blob. Atomic;
    /// checks the blocked-by-deps rule.
    async fn complete_agent_task(
        &self,
        agent_task_id: String,
        result_blob: String,
    ) -> Result<AgentTask, AgentError>;

    async fn link_agent_task_to_session(
        &self,
        agent_task_id: String,
        session_id: String,
    ) -> Result<AgentTask, AgentError>;

    /// All edges where either endpoint belongs to `queue_id`.
    async fn list_agent_task_links(
        &self,
        queue_id: String,
    ) -> Result<Vec<AgentTaskLink>, AgentError>;
}
