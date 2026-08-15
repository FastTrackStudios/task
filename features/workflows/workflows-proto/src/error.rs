//! `WorkflowError` — trait-boundary error type for workflows.

use thiserror::Error;

/// Errors that can flow back from a [`Workflow`](crate) implementation
/// or the `Orchestrator` runtime. Kept small + concrete so callers
/// can pattern-match without a `Box<dyn Error>` escape hatch.
#[derive(Debug, Error)]
pub enum WorkflowError {
    /// The acting agent isn't permitted to invoke the requested
    /// verb. Carries the verb name so callers can log it.
    #[error("forbidden: {actor} cannot perform `{verb}`")]
    Forbidden { actor: String, verb: String },

    /// The target session id doesn't exist.
    #[error("session not found: {0}")]
    SessionNotFound(uuid::Uuid),

    /// The target handoff id doesn't exist.
    #[error("handoff not found: {0}")]
    HandoffNotFound(uuid::Uuid),

    /// The requested transition is invalid for the current state.
    #[error("invalid transition: cannot move from `{from}` to `{to}`")]
    InvalidTransition { from: String, to: String },

    /// The subject already has an active session under another actor.
    /// Carries the conflicting actor so the caller can render a
    /// helpful message.
    #[error("subject already claimed by `{actor}`")]
    AlreadyClaimed { actor: String },

    /// The wrapped backend (DB, vox, …) failed. The message is
    /// human-readable; structured backend errors live in the
    /// backend crates.
    #[error("backend error: {0}")]
    Backend(String),

    /// JSON encode/decode failed on a payload field.
    #[error("payload codec error: {0}")]
    PayloadCodec(#[from] serde_json::Error),
}
