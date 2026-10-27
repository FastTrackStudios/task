//! Trait-boundary error type for [`crate::AppDocFaces`].

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(C)]
pub enum FacesError {
    /// The catalog holds no doc with this id.
    #[error("no app doc `{0}` in this org's catalog")]
    NotFound(String),

    /// The doc was deleted: its row is a tombstone and its content is
    /// no longer served.
    #[error("app doc `{0}` was deleted")]
    Deleted(String),

    #[error("backend: {0}")]
    Backend(String),
}
