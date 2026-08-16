//! The org lane's backend — one per org, mounted on that org's router.
//!
//! The org's identity is a field, never an argument: a caller on this
//! lane cannot name another org any more than a `files` caller can name a
//! path outside its own area. Everything it can see is filtered through
//! that org's grants.

use std::sync::Arc;

use files_storage_proto::{
    GrantUsage, RootPlacement, StorageError, StorageEvent, StorageGrantInfo, StorageLocationInfo,
    StorageService,
};
use uuid::Uuid;

use crate::core::StorageCore;
use crate::error::panicked;

#[derive(Clone, architect::HasDispatcher)]
pub struct StorageBackend {
    core: Arc<StorageCore>,
    org: String,
    /// This org's own hub, cloned from the coordinator's per-org map —
    /// `#[subscribe]` hands out a `&PubSub`, and clones share one
    /// subscriber list, so a per-org hub is what keeps one org's
    /// placements out of another's stream.
    events: architect::PubSub<StorageEvent>,
}

impl std::fmt::Debug for StorageBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageBackend")
            .field("org", &self.org)
            .finish_non_exhaustive()
    }
}

impl StorageBackend {
    #[must_use]
    pub fn new(core: Arc<StorageCore>, org: impl Into<String>) -> Self {
        let org = org.into();
        let events = core.org_hub(&org);
        Self { core, org, events }
    }

    #[must_use]
    pub fn org(&self) -> &str {
        &self.org
    }

    /// Authorize-free (the org IS the backend) blocking-pool hop. Every
    /// method goes through it, reads included: the registry is an
    /// in-memory structure behind a `std::sync::Mutex`, and taking that
    /// lock inline on a runtime worker is exactly the stall this seam
    /// exists to prevent (PR #284 review).
    async fn run<T, F>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(Arc<StorageCore>, String) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let core = self.core.clone();
        let org = self.org.clone();
        files_store::blocking(move || f(core, org), panicked).await
    }
}

impl StorageService for StorageBackend {
    async fn list_locations(&self) -> Result<Vec<StorageLocationInfo>, StorageError> {
        self.run(|core, org| Ok(core.locations_for(&org))).await
    }

    async fn list_grants(&self) -> Result<Vec<StorageGrantInfo>, StorageError> {
        self.run(|core, org| Ok(core.list_grants(Some(&org)))).await
    }

    async fn place_root(
        &self,
        root_id: Uuid,
        location_id: Uuid,
        relative_path: String,
    ) -> Result<RootPlacement, StorageError> {
        self.run(move |core, org| core.place_root(&org, root_id, location_id, &relative_path))
            .await
    }

    async fn placement(&self, root_id: Uuid) -> Result<RootPlacement, StorageError> {
        self.run(move |core, org| core.placement(&org, root_id))
            .await
    }

    async fn list_placements(&self) -> Result<Vec<RootPlacement>, StorageError> {
        self.run(|core, org| Ok(core.list_placements(&org))).await
    }

    async fn add_blob_replica(
        &self,
        root_id: Uuid,
        location_id: Uuid,
    ) -> Result<RootPlacement, StorageError> {
        self.run(move |core, org| core.add_blob_replica(&org, root_id, location_id))
            .await
    }

    async fn refresh_usage(&self, root_id: Uuid) -> Result<RootPlacement, StorageError> {
        self.run(move |core, org| core.refresh_usage(&org, root_id))
            .await
    }

    async fn usage(&self, location_id: Uuid) -> Result<GrantUsage, StorageError> {
        self.run(move |core, org| core.usage(&org, location_id))
            .await
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host this
/// org's hub. Publishing happens in [`StorageCore`], on every successful
/// mutation.
impl files_storage_proto::service::org::StorageServiceStreamSource for StorageBackend {
    fn events_hub(&self) -> &architect::PubSub<StorageEvent> {
        &self.events
    }
}
