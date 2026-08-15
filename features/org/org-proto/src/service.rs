//! Server-wide org-management RPC.
//!
//! [`OrgManagementService`] runs at `/server/vox` (one
//! endpoint per task-server process, not per-org). It lets a
//! signed-in CLI ask the server to scaffold a new on-disk org
//! under `<data_root>/orgs/<slug>/` and immediately start
//! serving it — without re-running `task org init` locally
//! against the server's filesystem.
//!
//! ## Authorization
//!
//! - **Bootstrap mode**: when zero orgs exist on disk, the
//!   first `create_org` request is accepted unauthenticated.
//!   Whichever client claims this slot becomes the home org's
//!   owner (signup follows separately).
//! - **Normal mode**: `session_token` must be a valid session
//!   issued by the home org's `auth.sqlite`. Any user signed
//!   into the home org can mint new federated orgs on this
//!   server.
//!
//! See `plans/federated-task-platform.md` Phase 3.

use facet::Facet;
use thiserror::Error;

use crate::OrgManifest;

/// Trait-boundary error type. Variants stay flat so the same
/// enum travels cleanly over vox.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum OrgManagementError {
    /// Slug didn't pass `[a-z0-9-]` 1-64 chars validation.
    #[error("invalid slug `{0}`")]
    InvalidSlug(String),
    /// An org with this slug already lives on disk under
    /// `<data_root>/orgs/`.
    #[error("org `{0}` already exists")]
    AlreadyExists(String),
    /// Caller's session token isn't valid against the home
    /// org's `auth.sqlite`, or there's no home org and the
    /// server isn't in bootstrap mode.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// Server tried to mark a second org as `is_home = true`
    /// when one already exists. One home per data root.
    #[error("home org already exists ({0})")]
    HomeExists(String),
    /// Filesystem / DB failure on the server side. Strings the
    /// source so callers don't pull in `std::io::Error`.
    #[error("io: {0}")]
    Io(String),
    /// Catch-all for anything else (panicked migration, dropped
    /// connection, …). Free-form message.
    #[error("internal: {0}")]
    Internal(String),
}

/// Server-side org scaffold request. `session_token` is empty
/// only in bootstrap mode (see crate docs).
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct CreateOrgRequest {
    /// Session token from the home org. Empty string is
    /// accepted only when no orgs exist on disk yet.
    pub session_token: String,
    /// `[a-z0-9-]`, 1-64 chars, no leading/trailing `-`.
    pub slug: String,
    /// Human-facing display name. Free-form UTF-8.
    pub display_name: String,
    /// Mark this as the identity-anchor org. Only legal once
    /// per data root.
    pub is_home: bool,
}

/// Server-management surface. Mounted at `/server/vox`.
#[architect::rpc]
pub trait OrgManagementService {
    /// Scaffold a new org under `<data_root>/orgs/<slug>/`,
    /// create + migrate its per-org SQLite DBs, and hot-add it
    /// to the live dispatcher so the next request to
    /// `/org/<slug>/...` routes to the new state without a
    /// server restart.
    fn create_org(&self, req: CreateOrgRequest) -> Result<OrgManifest, OrgManagementError>;

    /// Enumerate every org currently hosted by this server.
    /// Equivalent to the data carried in
    /// `/.well-known/task-server.json`, returned as the wire
    /// `OrgManifest` for callers that want the full record.
    fn list_orgs(&self) -> Result<Vec<OrgManifest>, OrgManagementError>;
}
