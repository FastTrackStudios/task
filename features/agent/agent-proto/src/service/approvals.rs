//! Mid-turn permission gating.

use crate::approval::{Approval, ApprovalDecision};
use crate::error::AgentError;

#[architect::rpc]
pub trait Approvals {
    fn list_pending_approvals(&self, session_id: &str) -> Result<Vec<Approval>, AgentError>;
    fn resolve_approval(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<Approval, AgentError>;
}
