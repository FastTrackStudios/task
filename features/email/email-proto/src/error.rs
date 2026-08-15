//! Trait-boundary error for every [`crate::EmailSync`] method.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum EmailSyncError {
    #[error("unknown account")]
    UnknownAccount,
    #[error("not found")]
    NotFound,
    #[error("authentication failed")]
    Auth,
    #[error("authorization denied: {0}")]
    Forbidden(String),
    #[error("network: {0}")]
    Network(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("conflict (server etag {server_etag})")]
    Conflict { server_etag: String },
    #[error("unsupported by this backend: {0}")]
    Unsupported(String),
    #[error("io: {0}")]
    Io(String),
    #[error("internal: {0}")]
    Internal(String),
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::EmailSyncError;
    unsafe impl vox_types::Reborrow for EmailSyncError {
        type Ref<'a> = EmailSyncError;
    }
}
