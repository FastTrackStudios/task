//! `LocationsService` — wire surface for reading + mutating
//! locations, decorated with `#[architect::rpc]`.
//!
//! The macro derives the async vox face from this sync trait:
//! backends impl `LocationsService` directly (zero-cost
//! in-process calls), and remote callers reach the same
//! surface via the auto-emitted client over vox. That's how
//! agent integrations talk to the locations store.
//!
//! Backends carry whatever state they need (the file-backed
//! `locations::Store` holds a `vault::Vault` behind a mutex) and
//! additionally implement [`architect::HasDispatcher`] so the
//! bridge knows how to marshal sync method calls onto the
//! right thread.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::Location;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum LocationsError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait LocationsService {
    /// Every location currently in the vault.
    fn list(&self) -> Result<Vec<Location>, LocationsError>;

    /// One location by id. `NotFound` when the id is unknown.
    fn get(&self, id: &str) -> Result<Location, LocationsError>;

    /// Create a new location. The backend assigns
    /// `loc.path` if empty (default `locations/<slug>.md`)
    /// and `loc.id` if nil. Conflict on existing path.
    fn create(&self, loc: Location) -> Result<Location, LocationsError>;

    /// Replace the location whose `id` matches. `NotFound`
    /// when the id is unknown. `loc.path` may not change —
    /// rename via [`Self::rename`].
    fn update(&self, loc: Location) -> Result<Location, LocationsError>;

    /// Move the backing file to a new vault-relative path.
    /// Preserves `id` so cross-feature references survive.
    fn rename(&self, id: &str, new_path: &str) -> Result<Location, LocationsError>;

    /// Remove the backing file. `NotFound` if the id is
    /// already gone.
    fn delete(&self, id: &str) -> Result<(), LocationsError>;
}
