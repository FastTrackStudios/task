//! Trait-boundary error type for every `git-proto` method.

use facet::Facet;
use thiserror::Error;

/// Backends translate forge-specific errors (octocrab's
/// `octocrab::Error`, `forgejo_api::ApiError`, network IO,
/// rate-limit / auth failures) into one of these variants so
/// callers don't depend on a specific HTTP client.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum GitError {
    /// Repo / issue / PR not found, or caller lacks visibility.
    #[error("not found: {0}")]
    NotFound(String),

    /// Auth missing or rejected. 401, 403, missing
    /// installation, expired token.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// Forge returned 429 or its equivalent.
    #[error("rate limited: {0}")]
    RateLimited(String),

    /// Forge-side error (5xx, transport hiccup, malformed
    /// response).
    #[error("forge: {0}")]
    Forge(String),

    /// Caller passed a value the forge or proto rejects
    /// (empty title, unknown merge method for this backend).
    #[error("invalid: {0}")]
    Invalid(String),

    /// Backend understands the request shape but doesn't
    /// implement it for this forge (e.g. draft PRs on a forge
    /// that lacks them). Treat as a hint to ask another
    /// backend, not a hard failure.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::GitError;
    unsafe impl vox_types::Reborrow for GitError {
        type Ref<'a> = GitError;
    }
}
