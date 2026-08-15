//! Trait-boundary error for every [`crate::VaultSync`] method.

use facet::Facet;
use thiserror::Error;

/// What can go wrong on a vault operation.
///
/// `Conflict` is the load-bearing variant: it carries the
/// server's current sha + bytes inline so callers can resolve
/// the divergence without a second round-trip — typically by
/// running a 3-way merge against their local copy.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum VaultSyncError {
    #[error("not found")]
    NotFound,
    #[error("bad path")]
    BadPath,
    #[error("conflict (server sha {server_sha})")]
    Conflict {
        server_sha: String,
        server_bytes: Vec<u8>,
    },
    #[error("io: {0}")]
    Io(String),
    #[error("internal: {0}")]
    Internal(String),
}
