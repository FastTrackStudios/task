//! Tool-call audit — query the typed tool-call history
//! distilled from message content.

use crate::error::AgentError;
use crate::tool::ToolCall;

#[architect::rpc]
pub trait ToolCalls {
    fn list_tool_calls(&self, session_id: &str) -> Result<Vec<ToolCall>, AgentError>;
    fn read_tool_call(&self, tool_call_id: &str) -> Result<ToolCall, AgentError>;
}
