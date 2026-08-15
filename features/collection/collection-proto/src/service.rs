//! `CollectionService` — the wire surface for the ordered-collection store.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{Collection, CollectionKind};
use links_proto::NodeRef;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum CollectionError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

/// Where to place an item — `after` a given node, or at the head when
/// `None`. Kept as a small request struct so `add_item` / `reorder` stay
/// within the 4-param RPC budget while carrying the collection id, the
/// item's node, and the anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct Placement {
    /// The collection to mutate.
    pub collection_id: String,
    /// The item's node reference.
    pub node: NodeRef,
    /// Place the item immediately after this node. `None` appends to the
    /// tail (for both `add_item` and `reorder`).
    #[serde(default)]
    pub after: Option<NodeRef>,
}

#[architect::rpc]
pub trait CollectionService {
    /// Create an empty collection. The backend assigns `id` if empty.
    fn create(
        &self,
        org: String,
        title: String,
        kind: CollectionKind,
    ) -> Result<Collection, CollectionError>;

    /// One collection by id, or `None` if absent.
    fn get(&self, id: &str) -> Result<Option<Collection>, CollectionError>;

    /// Every collection in `org`, optionally filtered to a single `kind`.
    fn list(
        &self,
        org: String,
        kind: Option<CollectionKind>,
    ) -> Result<Vec<Collection>, CollectionError>;

    /// Add `node` to the collection, assigning a lexorank between the
    /// `after` node and its successor (or appending when `after` is
    /// `None`). Returns the updated collection.
    fn add_item(&self, placement: Placement) -> Result<Collection, CollectionError>;

    /// Remove the item pointing at `node`. `NotFound` if absent.
    fn remove_item(
        &self,
        collection_id: &str,
        node: NodeRef,
    ) -> Result<Collection, CollectionError>;

    /// Move the item pointing at `placement.node` to sit after
    /// `placement.after` (or to the tail when `after` is `None`). Only
    /// that item's rank is recomputed.
    fn reorder(&self, placement: Placement) -> Result<Collection, CollectionError>;
}
