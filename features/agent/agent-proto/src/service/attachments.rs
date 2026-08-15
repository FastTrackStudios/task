//! File / image attachments. Off-message store keyed by
//! sha256.

use crate::attachment::{Attachment, AttachmentRef};
use crate::error::AgentError;

#[architect::rpc]
pub trait Attachments {
    fn upload_attachment(
        &self,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<AttachmentRef, AgentError>;
    fn read_attachment(&self, attachment_id: &str) -> Result<Attachment, AgentError>;
    fn list_attachments(&self, session_id: &str) -> Result<Vec<AttachmentRef>, AgentError>;
}
