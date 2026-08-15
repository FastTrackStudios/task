//! Service-level errors. Kept small + structural so the wire shape
//! stays predictable across backends. Mirrors
//! `scheduling-proto::SchedulingError`.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, Error, Facet)]
#[repr(u8)]
pub enum InboxError {
    #[error("not found: {id}")]
    NotFound { id: String },
    /// Validation failure on a payload field. `field` is a
    /// dotted-path (`item.created`).
    #[error("invalid {field}: {reason}")]
    Invalid { field: String, reason: String },
    /// Backend storage / vault IO bubbled up. Free-form message
    /// because the underlying error is implementation-defined.
    #[error("backend: {message}")]
    Backend { message: String },
}
