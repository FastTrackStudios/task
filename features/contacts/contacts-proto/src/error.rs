//! Service-level errors. Kept small + structural so the wire shape
//! stays predictable across backends. Mirrors `recall_proto::RecallError`.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, Error, Facet)]
#[repr(u8)]
pub enum ContactsError {
    #[error("not found: {id}")]
    NotFound { id: String },
    /// Validation failure on a payload field. `field` is a
    /// dotted-path (`contact.full_name`).
    #[error("invalid {field}: {reason}")]
    Invalid { field: String, reason: String },
    /// Backend storage / vault IO bubbled up. Free-form message
    /// because the underlying error is implementation-defined.
    #[error("backend: {message}")]
    Backend { message: String },
    /// A CardDAV sync failed (network, auth, or a malformed server
    /// response). Free-form so any provider's failure fits.
    #[error("sync: {message}")]
    Sync { message: String },
}
