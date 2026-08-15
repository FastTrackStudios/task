//! Trait-boundary error type for [`crate::Notify`].

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub enum NotifyError {
    #[error("notification not found: {0}")]
    NotFound(String),

    #[error("invalid: {0}")]
    Invalid(String),

    #[error("backend: {0}")]
    Backend(String),
}
