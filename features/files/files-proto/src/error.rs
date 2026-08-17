//! The Files error surface.
//!
//! The v1 error had four variants, every one carrying a `String`:
//! `NotFound(String)`, `AlreadyExists(String)`, `BadRequest(String)`,
//! `Io(String)`. A caller wanting to distinguish "this root is gone" from
//! "this path is gone" had to parse prose, and a UI wanting to offer
//! keep-both / replace / keep-existing on a collision had no way to know a
//! collision was what happened.
//!
//! These variants carry what the caller needs to act. Prose is for the
//! human; the shape is for the code.

use facet::Facet;
use thiserror::Error;

use crate::id::{DeviceId, GrantId, ReviewId, RootId, UploadId, VersionId};
use crate::path::{PathError, RootPath, TreePath};

/// What went wrong, in terms a caller can branch on.
#[derive(Debug, Clone, PartialEq, Facet, Error)]
#[repr(u8)]
pub enum FilesFault {
    // ── Identity ────────────────────────────────────────────────────
    #[error("no such root: {0}")]
    RootNotFound(RootId),
    #[error("no such path in root: {0}")]
    PathNotFound(RootPath),
    #[error("no such path in the org tree: {0}")]
    TreePathNotFound(TreePath),
    #[error("no such version: {0}")]
    VersionNotFound(VersionId),
    #[error("no such upload: {0}")]
    UploadNotFound(UploadId),
    #[error("no such review: {0}")]
    ReviewNotFound(ReviewId),
    #[error("no such device: {0}")]
    DeviceNotFound(DeviceId),

    // ── Collision ───────────────────────────────────────────────────
    /// The target already exists. The caller chooses how to proceed —
    /// see `files.write.upload`, which forbids choosing for them.
    #[error("already exists: {path}")]
    Exists { path: RootPath },
    /// A root already exists at this filesystem location.
    #[error("already a root: {0}")]
    AlreadyRoot(String),

    // ── Refusal ─────────────────────────────────────────────────────
    #[error("path is not valid: {0}")]
    BadPath(#[from] PathError),
    #[error("expected a directory: {0}")]
    NotADirectory(RootPath),
    #[error("expected a file: {0}")]
    NotAFile(RootPath),
    /// A move or copy whose destination lies inside its own source.
    #[error("cannot move {from} into itself at {to}")]
    IntoSelf { from: RootPath, to: RootPath },
    #[error("{0}")]
    Invalid(String),

    // ── Authorisation ───────────────────────────────────────────────
    /// The principal holds no grant conveying this action here. Distinct
    /// from `PathNotFound` deliberately: outside a granted subtree paths
    /// are *absent*, per `files.access.granularity`, so callers that must
    /// not leak existence get `PathNotFound` instead.
    #[error("not permitted: {action} on {path}")]
    Denied { action: String, path: RootPath },
    #[error("grant revoked: {0}")]
    GrantRevoked(GrantId),

    // ── Availability ────────────────────────────────────────────────
    /// The content exists and its holding location cannot be reached.
    /// Not an error about the tree — structure is still correct, per
    /// `files.catalogue.offline`.
    #[error("content unavailable: {path} is held on an unreachable location")]
    Unavailable { path: RootPath },
    /// The operation needs bytes this device does not hold and cannot
    /// fetch right now.
    #[error("not hydrated: {0}")]
    NotHydrated(RootPath),

    // ── Concurrency ─────────────────────────────────────────────────
    /// The precondition failed — someone else changed it first.
    #[error("stale: {path} changed since {expected}")]
    Stale { path: RootPath, expected: String },

    // ── Below us ────────────────────────────────────────────────────
    #[error("io: {0}")]
    Io(String),
    #[error("version store: {0}")]
    Store(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl FilesFault {
    /// Whether retrying unchanged could plausibly succeed. Drives client
    /// backoff without every caller re-deriving the list.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Unavailable { .. } | Self::NotHydrated(_) | Self::Io(_)
        )
    }

    /// Whether this is the caller's fault rather than ours.
    #[must_use]
    pub fn is_client_error(&self) -> bool {
        !matches!(self, Self::Io(_) | Self::Store(_) | Self::Internal(_))
    }
}

/// Convenience constructors for the noisier variants.
impl FilesFault {
    pub fn denied(action: impl Into<String>, path: RootPath) -> Self {
        Self::Denied {
            action: action.into(),
            path,
        }
    }

    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::Invalid(reason.into())
    }

    pub fn io(err: impl std::fmt::Display) -> Self {
        Self::Io(err.to_string())
    }

    pub fn store(err: impl std::fmt::Display) -> Self {
        Self::Store(err.to_string())
    }
}
