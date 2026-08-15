//! File-backed [`LocationsService`] implementation.
//!
//! CRUD is [`vault_entity::VaultEntityStore`], which wraps the
//! `vault::Vault` (the in-memory snapshot of pages on disk) behind a
//! mutex so concurrent `&self` calls serialize. Cheap to `Clone` —
//! internals are `Arc`'d. What stays here is `rename`, the one
//! operation that isn't plain CRUD. Each method re-scans the cached
//! vault snapshot (cheap) rather than maintaining a parallel
//! `HashMap<Uuid, Location>` index — the source of truth is the
//! markdown on disk.

use uuid::Uuid;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::Locations;
use crate::model::Location;
use crate::parse::{looks_like_location, parse_page};
use crate::service::{LocationsError, LocationsService};

vault_entity::entity_error_bridge!(LocationsError);

/// File-backed locations store. One per vault.
#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: VaultEntityStore<Locations>,
}

impl Store {
    #[must_use]
    pub fn new(vault: Vault) -> Self {
        Self {
            inner: VaultEntityStore::new(vault),
        }
    }

    /// Hand the inner `Vault` to a closure for direct
    /// inspection (read-only). Useful when other features
    /// (e.g. `inventory`) want to share the same snapshot.
    pub fn with_vault<R>(&self, f: impl FnOnce(&Vault) -> R) -> R {
        self.inner.with_vault(f)
    }
}

impl LocationsService for Store {
    fn list(&self) -> Result<Vec<Location>, LocationsError> {
        Ok(self.inner.list())
    }

    fn get(&self, id: &str) -> Result<Location, LocationsError> {
        self.inner.get(id).map_err(from_entity_error)
    }

    fn create(&self, loc: Location) -> Result<Location, LocationsError> {
        self.inner.create(loc).map_err(from_entity_error)
    }

    fn update(&self, loc: Location) -> Result<Location, LocationsError> {
        self.inner.update(loc).map_err(from_entity_error)
    }

    /// Move a page to `new_path`, keeping its bytes verbatim.
    ///
    /// Not plain CRUD — the shared store deliberately never moves a
    /// file on update — so it stays hand-written here.
    fn rename(&self, id: &str, new_path: &str) -> Result<Location, LocationsError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| LocationsError::BadRequest(format!("id: {e}")))?;
        self.inner.with_vault_mut(|guard| -> Result<Location, LocationsError> {
            let idx = guard
                .pages
                .iter()
                .position(|p| {
                    looks_like_location(p) && parse_page(p).map(|l| l.id == uuid).unwrap_or(false)
                })
                .ok_or_else(|| LocationsError::NotFound(id.to_string()))?;
            if guard.pages.iter().any(|p| p.rel_path == new_path) {
                return Err(LocationsError::AlreadyExists(new_path.to_string()));
            }
            let old_path = guard.pages[idx].rel_path.clone();
            let raw = guard.pages[idx].raw.clone();
            vault::delete_page(&mut *guard, &old_path).map_err(map_io)?;
            vault::create_page(&mut *guard, new_path, raw).map_err(map_io)?;
            let new_page = guard
                .pages
                .iter()
                .find(|p| p.rel_path == new_path)
                .ok_or_else(|| LocationsError::Io("rename: page missing post-write".into()))?;
            parse_page(new_page).map_err(|e| LocationsError::Io(e.to_string()))
        })
    }

    fn delete(&self, id: &str) -> Result<(), LocationsError> {
        self.inner.delete(id).map_err(from_entity_error)
    }
}

fn map_io(e: impl std::fmt::Display) -> LocationsError {
    LocationsError::Io(e.to_string())
}
