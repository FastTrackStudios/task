//! Full message payload returned by
//! [`crate::EmailSync::fetch_message`]. Bodies live here;
//! attachments are returned as metadata only — fetch the
//! bytes via [`crate::EmailSync::fetch_attachment`].

use crate::{Envelope, draft::AttachmentMeta};
use facet::Facet;

/// RFC 2822 Message-ID, stored verbatim — backends may include
/// or omit the angle brackets; helpers should tolerate both.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet)]
pub struct MessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet)]
pub struct ThreadId(pub String);

// serde alongside Facet: the offline read cache stores fetched
// messages as JSON on the client, and Facet is the wire codec, not a
// persistence format.
#[derive(Debug, Clone, Facet, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub envelope: Envelope,
    pub headers_raw: String,
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub attachments: Vec<AttachmentMeta>,
    /// `In-Reply-To` followed by `References`, in header order.
    pub references: Vec<String>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Message, MessageId, ThreadId};
    unsafe impl vox_types::Reborrow for Message {
        type Ref<'a> = Message;
    }
    unsafe impl vox_types::Reborrow for MessageId {
        type Ref<'a> = MessageId;
    }
    unsafe impl vox_types::Reborrow for ThreadId {
        type Ref<'a> = ThreadId;
    }
}
