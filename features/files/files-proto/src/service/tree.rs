//! Reading the namespace, and the replicated catalogue —
//! `files.catalogue.*`, `files.live.*`.
//!
//! Structure replicates everywhere; content replicates selectively. This
//! lane is the structure half: what exists, of what kind, of what size,
//! held where. It is authoritative about structure and about nothing
//! else — the moment it also caches content, `files.catalogue.bounded`
//! breaks and it has become a second storage system.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::{ContentId, RootId};
use crate::model::{BrowseEntry, TreeNode};
use crate::path::{RootPath, TreePath};

/// Whether an entry's bytes are here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Hydration {
    /// Bytes are resident and readable now.
    Resident,
    /// A pointer stub: the path exists with correct name, size and
    /// metadata, and bytes arrive on access.
    Stub,
    /// Bytes exist but their holding location is unreachable. Visibly
    /// unavailable — never silently missing, and never an empty folder.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum EntryKind {
    File,
    Directory,
    /// A child project or root mounted here, whose content may live
    /// elsewhere entirely.
    Mount,
}

/// One catalogue entry.
///
/// Carries structure and nothing heavier: no content, no renditions, no
/// extracted text. Roughly 200–300 bytes on the wire, which is what keeps
/// a 100k-file tree inside a phone's budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct CatalogueEntry {
    pub root_id: RootId,
    pub path: RootPath,
    pub kind: EntryKind,
    pub size: u64,
    /// `None` while adoption is still hashing — the entry is published
    /// and browsable, just unverified.
    pub content: Option<ContentId>,
    pub hydration: Hydration,
    /// Locations holding these bytes. More than one after replication;
    /// empty for a directory.
    pub locations: Vec<String>,
    pub modified_at: DateTime<Utc>,
    /// When this entry's structure was last confirmed against its
    /// holding location.
    ///
    /// The catalogue is allowed to be stale. It is not allowed to look
    /// current when it is not — `files.catalogue.staleness`.
    pub confirmed_at: DateTime<Utc>,
}

/// An opaque position in the change stream. Hand it back to
/// [`TreeService::changes_since`] to resume without re-listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[serde(transparent)]
#[repr(C)]
pub struct Cursor(pub String);

/// A page of catalogue changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct CatalogueDelta {
    pub changed: Vec<CatalogueEntry>,
    pub removed: Vec<RootPath>,
    pub cursor: Cursor,
    /// More waiting. Call again rather than assuming this page is all.
    pub more: bool,
}

/// How current a client's view of a root is, and of what.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Freshness {
    pub root_id: RootId,
    /// Last time this root's holding location answered.
    pub confirmed_at: DateTime<Utc>,
    /// False when the location is unreachable — the signal a view needs
    /// to say "as of" rather than implying now.
    pub reachable: bool,
    pub entries: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum TreeEvent {
    /// Entries were added or changed. Batched: one event may carry many.
    Changed(CatalogueDelta),
    /// A location's reachability changed, so every entry it holds
    /// changed hydration without any of them changing content.
    FreshnessChanged(Freshness),
}

#[architect::rpc]
pub trait TreeService {
    /// Direct children of `path` inside a root's live tree. Empty path
    /// lists the root.
    async fn browse(&self, root_id: RootId, path: RootPath)
    -> Result<Vec<BrowseEntry>, FilesFault>;

    /// Resolve one path of the org tree — the unified namespace over
    /// Projects, Vault, Wiki and Assets. `""` lists the areas.
    ///
    /// The same resolver backs the explorer and the WebDAV mount, so the
    /// app and a mounted share always show one tree.
    async fn resolve(&self, path: TreePath) -> Result<TreeNode, FilesFault>;

    /// One entry's catalogue record, without listing its parent.
    async fn entry(&self, root_id: RootId, path: RootPath) -> Result<CatalogueEntry, FilesFault>;

    /// A root's full catalogue, paged. The initial sync; afterwards use
    /// [`Self::changes_since`].
    async fn catalogue(
        &self,
        root_id: RootId,
        cursor: Option<Cursor>,
    ) -> Result<CatalogueDelta, FilesFault>;

    /// Everything that changed since `cursor`.
    ///
    /// A client reconnecting after a gap converges from here — it never
    /// re-lists the tree to catch up.
    async fn changes_since(
        &self,
        root_id: RootId,
        cursor: Cursor,
    ) -> Result<CatalogueDelta, FilesFault>;

    /// How current this view is, per root. What a UI reads to say "as
    /// of" instead of implying now.
    async fn freshness(&self) -> Result<Vec<Freshness>, FilesFault>;
}
