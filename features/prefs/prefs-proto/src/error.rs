//! Trait-boundary error type for [`crate::PrefsService`].

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(C)]
pub enum PrefsError {
    #[error("invalid: {0}")]
    Invalid(String),

    #[error("backend: {0}")]
    Backend(String),
}
