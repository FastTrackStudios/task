//! File-backed [`BodyService`] impl.
//!
//! CRUD is [`vault_entity::VaultEntityStore`]; only `find_by_kind` and
//! `log_entry` — the parts specific to body metrics — live here.

use std::sync::{Arc, Mutex};

use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::BodyMetrics;
use crate::model::{BodyEntry, BodyMetric};
use crate::service::{BodyError, BodyService};

vault_entity::entity_error_bridge!(BodyError);

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: VaultEntityStore<BodyMetrics>,
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

impl BodyService for Store {
    fn list(&self) -> Result<Vec<BodyMetric>, BodyError> {
        Ok(self.inner.list())
    }

    fn get(&self, id: &str) -> Result<BodyMetric, BodyError> {
        self.inner.get(id).map_err(from_entity_error)
    }

    fn find_by_kind(&self, kind: &str) -> Result<BodyMetric, BodyError> {
        let needle = kind.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Err(BodyError::BadRequest("empty kind".into()));
        }
        self.inner
            .find(|m| m.kind.eq_ignore_ascii_case(&needle))
            .ok_or_else(|| BodyError::NotFound(format!("kind: {kind}")))
    }

    fn create(&self, metric: BodyMetric) -> Result<BodyMetric, BodyError> {
        self.inner.create(metric).map_err(from_entity_error)
    }

    fn update(&self, metric: BodyMetric) -> Result<BodyMetric, BodyError> {
        self.inner.update(metric).map_err(from_entity_error)
    }

    fn delete(&self, id: &str) -> Result<(), BodyError> {
        self.inner.delete(id).map_err(from_entity_error)
    }

    fn log_entry(&self, metric_id: &str, mut entry: BodyEntry) -> Result<BodyMetric, BodyError> {
        if entry.id.is_nil() {
            entry.id = uuid::Uuid::new_v4();
        }
        let mut metric = self.get(metric_id)?;
        metric.entries.push(entry);
        self.update(metric)
    }
}
