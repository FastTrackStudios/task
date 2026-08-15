//! Trait-boundary error type for [`crate::ThreadsService`].

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(C)]
pub enum ThreadsError {
    #[error("thread not found: {0}")]
    ThreadNotFound(String),

    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error("invalid: {0}")]
    Invalid(String),

    #[error("backend: {0}")]
    Backend(String),
}
