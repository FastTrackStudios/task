//! The one error type all three placement lanes speak (issue #262).

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// This org holds no grant on the location it tried to place on —
    /// the registry is deployment-scoped and admission is explicit.
    #[error("no storage grant: {0}")]
    NotGranted(String),
    /// The grant exists but does not carry the capability class the
    /// operation needs (a live tree on a blob-only grant, say).
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    /// The placement would push the org past its logical-byte quota on
    /// that location.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    /// The volume's agent has announced but the operator has not
    /// approved it, so its volumes are not registered locations.
    #[error("agent not approved: {0}")]
    AgentNotApproved(String),
    /// The caller did not prove it may do this: an operator call with no
    /// valid session, or an agent call whose credential does not match
    /// the agent it names. Agent ids and location ids are not secrets —
    /// they are handed out by `list_agents` / `list_locations` — so
    /// naming one is never, on its own, authorization to act as it
    /// (PR #284 review).
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("io: {0}")]
    Io(String),
}
