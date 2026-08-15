//! File-backed [`ExercisesService`] implementation.
//!
//! CRUD is [`vault_entity::VaultEntityStore`]; only `find_by_name` and
//! `rename` — the parts specific to the exercise catalog — live here.

use std::sync::{Arc, Mutex};

use uuid::Uuid;
use vault::Vault;
use vault_entity::VaultEntityStore;
use vault_entity::store::VaultEntity;

use crate::entity::Exercises;
use crate::model::Exercise;
use crate::service::{ExercisesError, ExercisesService};

vault_entity::entity_error_bridge!(ExercisesError);

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: VaultEntityStore<Exercises>,
}

impl Store {
    pub fn new(vault: Vault) -> Self {
        Self {
            inner: VaultEntityStore::new(vault),
        }
    }

    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        Self {
            inner: VaultEntityStore::from_shared(inner),
        }
    }

    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.shared()
    }
}

fn map_io(e: impl std::fmt::Display) -> ExercisesError {
    ExercisesError::Io(e.to_string())
}

impl ExercisesService for Store {
    fn list(&self) -> Result<Vec<Exercise>, ExercisesError> {
        Ok(self.inner.list())
    }

    fn get(&self, id: &str) -> Result<Exercise, ExercisesError> {
        self.inner.get(id).map_err(from_entity_error)
    }

    fn find_by_name(&self, name: &str) -> Result<Exercise, ExercisesError> {
        let needle = name.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Err(ExercisesError::BadRequest("empty name".into()));
        }
        self.inner
            .find(|e| {
                e.name.eq_ignore_ascii_case(&needle)
                    || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(&needle))
            })
            .ok_or_else(|| ExercisesError::NotFound(format!("name: {name}")))
    }

    fn create(&self, exercise: Exercise) -> Result<Exercise, ExercisesError> {
        self.inner.create(exercise).map_err(from_entity_error)
    }

    fn update(&self, exercise: Exercise) -> Result<Exercise, ExercisesError> {
        self.inner.update(exercise).map_err(from_entity_error)
    }

    /// Move a page to `new_path`, keeping its bytes verbatim. The
    /// shared store never moves files, so this stays hand-written.
    fn rename(&self, id: &str, new_path: &str) -> Result<Exercise, ExercisesError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| ExercisesError::BadRequest(format!("id: {e}")))?;
        self.inner.with_vault_mut(|v| {
            let idx = v
                .pages
                .iter()
                .position(|p| {
                    Exercises::matches(p)
                        && Exercises::from_page(p)
                            .map(|e| e.id == uuid)
                            .unwrap_or(false)
                })
                .ok_or_else(|| ExercisesError::NotFound(id.to_string()))?;
            if v.pages.iter().any(|p| p.rel_path == new_path) {
                return Err(ExercisesError::AlreadyExists(new_path.to_string()));
            }
            let old_path = v.pages[idx].rel_path.clone();
            let raw = v.pages[idx].raw.clone();
            vault::delete_page(v, &old_path).map_err(map_io)?;
            vault::create_page(v, new_path, raw).map_err(map_io)?;
            let new_page = v
                .pages
                .iter()
                .find(|p| p.rel_path == new_path)
                .ok_or_else(|| ExercisesError::Io("rename: page missing post-write".into()))?;
            Exercises::from_page(new_page).map_err(|e| ExercisesError::Io(e.to_string()))
        })
    }

    fn delete(&self, id: &str) -> Result<(), ExercisesError> {
        self.inner.delete(id).map_err(from_entity_error)
    }
}
