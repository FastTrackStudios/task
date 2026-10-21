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
    /// A [`CollectionService::save`] refused: the collection is no
    /// longer at the revision the caller read. Carries the revision it
    /// is at now. Nothing was written — re-read it with `get`, show the
    /// person what changed, and save again against the new revision.
    #[error("stale: the collection moved on (now at revision {0})")]
    Stale(u64),
}

/// A whole-collection edit, made against the revision it was read at —
/// what a set-list editor sends when the person presses save.
///
/// `title` and `items` *replace* the collection's: `items` is the full
/// order, head first, exactly as the editor shows it. Kind, org and id
/// are not here because a save never changes what a collection is or
/// whose it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct CollectionSave {
    /// The collection to save over.
    pub id: String,
    /// The [`Collection::revision`] the caller read. The save lands only
    /// if the collection is still at it; there is no unconditional form,
    /// because an unconditional whole-list write is precisely how one
    /// person's set list silently replaces another's.
    pub revision: u64,
    /// The new title; blank is refused, as on `create` and `rename`.
    pub title: String,
    /// Every item, in order. A node may appear once; a collection may
    /// not come to contain itself.
    #[serde(default)]
    pub items: Vec<NodeRef>,
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

    /// Retitle one. The id, kind, org and every item stay as they are —
    /// a set list renamed the morning of the show is the same set list.
    /// An empty title is refused, as it is on `create`.
    fn rename(&self, id: &str, title: String) -> Result<Collection, CollectionError>;

    /// Remove the collection itself. The nodes it gathered are untouched:
    /// a collection is an ordering over things that live elsewhere, so
    /// deleting one deletes no song, no chart and no file.
    ///
    /// `NotFound` when there is no such collection — so a second delete
    /// says so rather than reporting success.
    fn delete(&self, id: &str) -> Result<(), CollectionError>;

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

    /// Save a whole collection — its title and its full order — against
    /// the revision the caller read. This is the safe save for a set
    /// list, a show or a library edited as one thing.
    ///
    /// Two people editing one set list each hold the revision they
    /// read; the first save lands and moves the revision on, and the
    /// second is [`CollectionError::Stale`] rather than an overwrite.
    /// Any change counts — a rename, or one item added through
    /// `add_item` — because the editor's list was made without it.
    ///
    /// `add_item`, `remove_item` and `reorder` stay unconditional on
    /// purpose: each names one item, so two of them from two people
    /// compose instead of one erasing the other. It is the whole-list
    /// write that loses work, and it is the one that is checked.
    ///
    /// Returns the saved collection, carrying its new revision.
    fn save(&self, edit: CollectionSave) -> Result<Collection, CollectionError>;
}
