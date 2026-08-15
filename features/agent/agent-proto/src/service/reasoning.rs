//! Extended-thinking access — distinct from message body
//! because some UIs collapse/redact it independently.

use crate::error::AgentError;
use crate::reasoning::ReasoningBlock;

#[architect::rpc]
pub trait Reasoning {
    fn read_reasoning(&self, message_id: &str) -> Result<Option<ReasoningBlock>, AgentError>;
}
