//! File-backed [`InventoryService`] implementation.
//!
//! CRUD is [`vault_entity::VaultEntityStore`]; what stays here is the
//! part that is specific to inventory — the location filter, the
//! path rename, and the status / condition / location setters. Each
//! call re-scans the cached vault rather than maintaining a parallel
//! index — the markdown on disk is the source of truth.

use std::sync::{Arc, Mutex};

use uuid::Uuid;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::Items;
use crate::model::Item;
use crate::parse::{looks_like_item, parse_page};
use crate::service::{InventoryError, InventoryService};

vault_entity::entity_error_bridge!(InventoryError);

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: VaultEntityStore<Items>,
}

impl Store {
    #[must_use]
    pub fn new(vault: Vault) -> Self {
        Self {
            inner: VaultEntityStore::new(vault),
        }
    }

    /// Reuse a `vault::Vault` mutex already owned by another
    /// feature (e.g. the locations store) so both surfaces
    /// share one consistent snapshot.
    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        Self {
            inner: VaultEntityStore::from_shared(inner),
        }
    }

    /// Expose the inner mutex for callers that want to share
    /// it with another feature's store.
    #[must_use]
    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.shared()
    }
}

impl InventoryService for Store {
    fn list(&self) -> Result<Vec<Item>, InventoryError> {
        Ok(self.inner.list())
    }

    fn list_at(&self, location_id: &str) -> Result<Vec<Item>, InventoryError> {
        let uuid = Uuid::parse_str(location_id)
            .map_err(|e| InventoryError::BadRequest(format!("location_id: {e}")))?;
        Ok(self.inner.with_vault(|v| crate::scan::items_at(v, uuid)))
    }

    fn get(&self, id: &str) -> Result<Item, InventoryError> {
        self.inner.get(id).map_err(from_entity_error)
    }

    fn create(&self, item: Item) -> Result<Item, InventoryError> {
        self.inner.create(item).map_err(from_entity_error)
    }

    fn update(&self, item: Item) -> Result<Item, InventoryError> {
        self.inner.update(item).map_err(from_entity_error)
    }

    /// Move a page to `new_path`, keeping its bytes verbatim.
    ///
    /// Not plain CRUD — the shared store deliberately never moves a
    /// file on update — so it stays hand-written here.
    fn rename(&self, id: &str, new_path: &str) -> Result<Item, InventoryError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| InventoryError::BadRequest(format!("id: {e}")))?;
        self.inner.with_vault_mut(|guard| -> Result<Item, InventoryError> {
            let idx = guard
                .pages
                .iter()
                .position(|p| {
                    looks_like_item(p) && parse_page(p).map(|i| i.id == uuid).unwrap_or(false)
                })
                .ok_or_else(|| InventoryError::NotFound(id.to_string()))?;
            if guard.pages.iter().any(|p| p.rel_path == new_path) {
                return Err(InventoryError::AlreadyExists(new_path.to_string()));
            }
            let old_path = guard.pages[idx].rel_path.clone();
            let raw = guard.pages[idx].raw.clone();
            vault::delete_page(&mut *guard, &old_path).map_err(map_io)?;
            vault::create_page(&mut *guard, new_path, raw).map_err(map_io)?;
            let new_page = guard
                .pages
                .iter()
                .find(|p| p.rel_path == new_path)
                .ok_or_else(|| InventoryError::Io("rename: page missing post-write".into()))?;
            parse_page(new_page).map_err(|e| InventoryError::Io(e.to_string()))
        })
    }

    fn delete(&self, id: &str) -> Result<(), InventoryError> {
        self.inner.delete(id).map_err(from_entity_error)
    }

    fn set_status(&self, id: &str, status: &str) -> Result<Item, InventoryError> {
        let mut item = self.get(id)?;
        item.status = status.to_string();
        self.update(item)
    }

    fn set_condition(&self, id: &str, condition: &str) -> Result<Item, InventoryError> {
        let mut item = self.get(id)?;
        item.condition = condition.to_string();
        self.update(item)
    }

    fn set_location(&self, id: &str, location_id: &str) -> Result<Item, InventoryError> {
        let mut item = self.get(id)?;
        item.location_id = if location_id.is_empty() {
            None
        } else {
            Some(
                Uuid::parse_str(location_id)
                    .map_err(|e| InventoryError::BadRequest(format!("location_id: {e}")))?,
            )
        };
        self.update(item)
    }
}

fn map_io(e: impl std::fmt::Display) -> InventoryError {
    InventoryError::Io(e.to_string())
}
