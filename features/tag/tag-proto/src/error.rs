//! Service-level errors. Small + structural so the wire shape stays
//! predictable across backends. Mirrors `inbox_proto::InboxError`.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, Error, Facet)]
#[repr(u8)]
pub enum TagError {
    #[error("not found: {id}")]
    NotFound { id: String },
    /// Validation failure on a payload field (`tag.name`).
    #[error("invalid {field}: {reason}")]
    Invalid { field: String, reason: String },
    /// Backend storage / vault IO bubbled up.
    #[error("backend: {message}")]
    Backend { message: String },
}
