//! Backend registry — the "which agent runtimes are wired
//! up" surface. Usually owned by whoever runs the server,
//! not by individual backends.

use crate::backend::{AgentBackend, BackendHealth, BackendKind};
use crate::error::AgentError;

#[architect::rpc]
pub trait Backends {
    /// Register a runner, or update an existing registration.
    ///
    /// The backend's [`crate::runner::RunnerProfile`] is what
    /// routing matches against; a capability outside the closed
    /// vocabulary is refused here rather than stored and silently
    /// never matched.
    async fn upsert_backend(&self, backend: AgentBackend) -> Result<AgentBackend, AgentError>;

    /// Deregister a runner.
    async fn remove_backend(&self, backend_id: String) -> Result<(), AgentError>;

    /// Every registered runner, live or stale.
    async fn list_backends(&self) -> Result<Vec<AgentBackend>, AgentError>;

    /// Whether a runner has heartbeated recently enough to be
    /// offered work.
    async fn backend_health(&self, backend_id: String) -> Result<BackendHealth, AgentError>;

    /// Say a runner is alive right now.
    ///
    /// Separate from `upsert_backend` because it is the hot call —
    /// every runner makes it on a timer, and it must not require
    /// re-sending the whole registration.
    async fn heartbeat_backend(&self, backend_id: String) -> Result<(), AgentError>;

    async fn backends_by_kind(&self, kind: BackendKind) -> Result<Vec<AgentBackend>, AgentError>;
}
