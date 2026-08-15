//! `InventoryService` — wire surface for reading + mutating
//! items, decorated with `#[architect::rpc]`.
//!
//! The macro derives the async vox face: backends impl
//! `InventoryService` directly (zero-cost in-process calls)
//! and remote callers — including agent integrations — reach
//! the same surface via the auto-emitted client over vox.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::Item;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum InventoryError {
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
pub trait InventoryService {
    /// Every item currently in the vault.
    fn list(&self) -> Result<Vec<Item>, InventoryError>;

    /// Every item whose `location_id` matches.
    fn list_at(&self, location_id: &str) -> Result<Vec<Item>, InventoryError>;

    /// One item by id.
    fn get(&self, id: &str) -> Result<Item, InventoryError>;

    /// Create a new item. The backend assigns `item.path`
    /// (`inventory/<slug>.md`) and `item.id` if either is
    /// empty / nil.
    fn create(&self, item: Item) -> Result<Item, InventoryError>;

    /// Replace the item whose `id` matches. Path is preserved;
    /// rename via [`Self::rename`].
    fn update(&self, item: Item) -> Result<Item, InventoryError>;

    /// Move the backing file to a new vault-relative path,
    /// preserving `id`.
    fn rename(&self, id: &str, new_path: &str) -> Result<Item, InventoryError>;

    /// Remove the backing file. `NotFound` if the id is gone.
    fn delete(&self, id: &str) -> Result<(), InventoryError>;

    /// Convenience mutator — set `status` without sending the
    /// whole `Item`. Returns the updated row.
    fn set_status(&self, id: &str, status: &str) -> Result<Item, InventoryError>;

    /// Convenience mutator — set `condition`.
    fn set_condition(&self, id: &str, condition: &str) -> Result<Item, InventoryError>;

    /// Convenience mutator — move an item to a new location.
    /// Pass `None` (empty string) to clear the location.
    fn set_location(&self, id: &str, location_id: &str) -> Result<Item, InventoryError>;
}
