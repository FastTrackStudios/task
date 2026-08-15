//! Live event subscription — one `#[subscribe]` stream served
//! from the backend's `architect::PubSub` hub.
//!
//! This replaced three Tx-parameter subscriptions
//! (`subscribe_session` / `subscribe_board` / `subscribe_global`).
//! Only the session-scoped one was ever implemented — board and
//! global closed the channel on sight in every backend — and its
//! server-side `session_id` filter is now the `session_id` on
//! [`AgentEventEnvelope`], applied client-side. The firehose the
//! dead `subscribe_global` wanted is what the one stream already
//! is.

use crate::event::AgentEventEnvelope;

#[architect::rpc]
pub trait Subscriptions {
    /// Every agent event this backend produces, as it happens —
    /// turn lifecycle, message + reasoning deltas, tool calls,
    /// approvals, questions, metering. Unfiltered across sessions;
    /// each envelope carries its `session_id` so a chat view keeps
    /// its own and a sidebar keeps them all. See
    /// [`AgentEventEnvelope`] for the fetch-once-then-fold
    /// subscriber contract.
    #[subscribe]
    fn events(&self) -> AgentEventEnvelope;
}
