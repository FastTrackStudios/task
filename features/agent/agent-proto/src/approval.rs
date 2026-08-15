//! Agent-initiated approval requests. The agent pauses
//! mid-turn and asks the user for permission before doing
//! something (run a command, write a file, send an email).
//!
//! Mirrors Hermes's `approval` SSE event + `CodexMonitor`'s
//! `RequestUserInputParams`. The flow:
//!
//! 1. Agent emits [`crate::event::AgentEvent::ApprovalRequested`].
//! 2. UI renders the prompt + options.
//! 3. User picks; UI calls [`crate::service::Agents::resolve_approval`].
//! 4. Backend forwards the decision; agent continues or
//!    aborts.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct Approval {
    pub id: String,
    pub session_id: String,
    /// Optional message id this approval is bound to
    /// (typically the tool-use block that prompted it).
    pub message_id: String,
    /// What the agent wants to do — backend-supplied
    /// short title (`"Run rm -rf …"`).
    pub title: String,
    /// Longer description. Markdown allowed.
    pub description: String,
    pub kind: ApprovalKind,
    /// Severity hint for the UI.
    pub risk: RiskLevel,
    pub created_at: DateTime<Utc>,
    /// Resolution; `None` while pending.
    pub decision: Option<ApprovalDecision>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum ApprovalKind {
    /// Run a shell command.
    Shell,
    /// Write / edit / delete a file.
    FileWrite,
    /// Make an external network request (web fetch, API).
    NetworkRequest,
    /// Send a message externally (email, Slack, ...).
    SendMessage,
    /// Spawn a sub-agent.
    Delegate,
    /// Anything else.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    /// Destructive operation that can't be undone.
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum ApprovalDecision {
    Approve,
    /// Approve and remember for the rest of the session.
    ApproveSession,
    /// Approve and remember globally for this profile.
    AlwaysApprove,
    Deny,
    /// Caller cancelled the turn entirely.
    CancelTurn,
}
