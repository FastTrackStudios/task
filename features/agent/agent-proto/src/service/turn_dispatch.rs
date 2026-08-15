//! Turn dispatch — the heart of any chat-capable agent.

use crate::attachment::AttachmentRef;
use crate::error::AgentError;
use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct DispatchTurn {
    pub session_id: String,
    /// User message text (markdown / plain).
    pub text: String,
    /// Attachments. Referenced by id; bytes must already
    /// be in the attachment store.
    pub attachments: Vec<AttachmentRef>,
    /// Override profile id. Empty = use the session's
    /// locked profile.
    pub profile_override_id: String,
    /// Override personality id. Empty = profile's active
    /// personality.
    pub personality_override_id: String,
    /// Override model id (provider-namespaced). Empty =
    /// profile default.
    pub model_override: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct DispatchAck {
    pub session_id: String,
    pub stream_id: String,
    pub turn_id: u64,
    pub started_at: DateTime<Utc>,
    pub effective_model: String,
    pub effective_backend_id: String,
    pub effective_profile_id: String,
}

#[architect::rpc]
pub trait TurnDispatch {
    /// Kick off a new turn. Returns immediately; events
    /// flow over the session's subscription channel
    /// (see `Subscriptions`).
    fn dispatch_turn(&self, args: DispatchTurn) -> Result<DispatchAck, AgentError>;

    /// Cancel the in-flight turn, if any.
    fn cancel_turn(&self, session_id: &str) -> Result<(), AgentError>;

    /// Resume a session that crashed mid-turn. Backends
    /// with a run journal replay missed events; others
    /// return `AgentError::Unsupported`-equivalent (or
    /// just fail gracefully).
    fn resume_session(&self, session_id: &str) -> Result<DispatchAck, AgentError>;
}
