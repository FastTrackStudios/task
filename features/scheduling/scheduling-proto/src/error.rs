//! Service-level errors. Kept small + structural so the wire shape
//! stays predictable across backends.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, Error, Facet)]
#[repr(u8)]
pub enum SchedulingError {
    #[error("not found: {id}")]
    NotFound { id: String },
    /// The requested slot conflicts with an existing booking, or
    /// falls outside every applicable [`crate::AvailabilityRule`].
    #[error("slot not available")]
    SlotUnavailable,
    /// Validation failure on a payload field. `field` is
    /// dotted-path (`event_type.duration_min`).
    #[error("invalid {field}: {reason}")]
    Invalid { field: String, reason: String },
    /// Backend storage / vault IO bubbled up. Free-form message
    /// because the underlying error is implementation-defined.
    #[error("backend: {message}")]
    Backend { message: String },
}
