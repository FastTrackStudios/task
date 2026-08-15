//! The org lane: what THIS org may see and place. The backend is
//! constructed per org, so the org's identity is the backend's — never a
//! caller-supplied argument (the same discipline `files`' `FilesBackend`
//! applies to paths: confinement is structural, not parameterised).

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StorageError;
use crate::model::{GrantUsage, RootPlacement, StorageGrantInfo, StorageLocationInfo};

/// Live-update payload for [`StorageService::events`]. Fetch current
/// state once via `list_locations`/`list_placements` (after subscribing,
/// so nothing is missed in between), then fold these in — the same
/// no-snapshot-variant contract as `files_proto::FilesEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum StorageEvent {
    /// A location this org can reach was registered, or its health
    /// changed.
    LocationChanged(StorageLocationInfo),
    /// A grant was issued to this org.
    GrantIssued(StorageGrantInfo),
    /// A grant of this org's was revoked. Payload is the grant id.
    GrantRevoked(Uuid),
    /// A root's placement changed — placed, hosted, replicated, or
    /// re-measured.
    PlacementChanged(RootPlacement),
}

#[architect::rpc]
pub trait StorageService {
    /// The locations this org holds a grant on. An ungranted location is
    /// invisible here, and placing on it fails with
    /// [`StorageError::NotGranted`] even if its id is guessed.
    async fn list_locations(&self) -> Result<Vec<StorageLocationInfo>, StorageError>;

    /// This org's grants.
    async fn list_grants(&self) -> Result<Vec<StorageGrantInfo>, StorageError>;

    /// Bind a root's live tree to a location and have that location's
    /// agent host it: create the tree under `<location root>/<grant
    /// prefix>/<relative_path>` and initialize the authoritative
    /// version-store repo inside it (ADR 0001 — the agent hosting the
    /// live tree owns the authoritative repo).
    ///
    /// Fails with [`StorageError::NotGranted`] without a grant,
    /// [`StorageError::CapabilityDenied`] if the grant lacks
    /// `LiveTrees`, [`StorageError::BadRequest`] if `relative_path`
    /// escapes the grant's prefix, and [`StorageError::AlreadyExists`]
    /// if the root already has a live tree.
    async fn place_root(
        &self,
        root_id: Uuid,
        location_id: Uuid,
        relative_path: String,
    ) -> Result<RootPlacement, StorageError>;

    /// One root's placement across both axes.
    async fn placement(&self, root_id: Uuid) -> Result<RootPlacement, StorageError>;

    /// Every placement this org holds.
    async fn list_placements(&self) -> Result<Vec<RootPlacement>, StorageError>;

    /// Replicate the root's version-store blobs onto a second location —
    /// the axis that is independent of the live tree. Requires a grant
    /// carrying `Blobs` on the destination; the destination need not be
    /// able to host live trees at all. Re-calling re-syncs.
    async fn add_blob_replica(
        &self,
        root_id: Uuid,
        location_id: Uuid,
    ) -> Result<RootPlacement, StorageError>;

    /// Re-measure the root's logical bytes from its authoritative repo
    /// and re-charge them against the grants involved. This is the point
    /// where quota usage becomes current.
    async fn refresh_usage(&self, root_id: Uuid) -> Result<RootPlacement, StorageError>;

    /// This org's quota position on one granted location.
    async fn usage(&self, location_id: Uuid) -> Result<GrantUsage, StorageError>;

    /// Location / grant / placement changes for this org, as they happen.
    #[subscribe]
    fn events(&self) -> StorageEvent;
}
