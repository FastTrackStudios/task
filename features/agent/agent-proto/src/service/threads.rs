//! Conversation history within a session — messages,
//! curator annotations.

use crate::error::AgentError;
use crate::message::Message;

#[architect::rpc]
pub trait Threads {
    /// List messages for a session, newest-first. `limit`
    /// caps the page; `before_cursor` paginates backward.
    fn list_messages(
        &self,
        session_id: &str,
        limit: u32,
        before_cursor: &str,
    ) -> Result<Vec<Message>, AgentError>;

    fn read_message(&self, message_id: &str) -> Result<Message, AgentError>;

    /// Append a free-form note to the session as a
    /// `System`-role message. Useful for curator
    /// annotations that don't go through the agent.
    fn append_note(&self, session_id: &str, text: &str) -> Result<Message, AgentError>;
}
