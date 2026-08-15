//! [`Collection`] — a generic, ordered list of [`NodeRef`] items.
//!
//! A song Library, a Setlist, a Show, and a Playlist are all the *same*
//! thing: an ordered collection of references to nodes that already live
//! elsewhere in the graph (songs, notes, videos, …). The only thing that
//! differs is the [`CollectionKind`]. Items are addressed by their
//! [`NodeRef`] and ordered by a fractional-index `rank` (a lexorank key)
//! so inserts and reorders stay local — no re-indexing of siblings.
//!
//! This is the wasm-clean model half; `rank` is a plain `String` and the
//! lexorank generator lives in the native `collection` crate.

use facet::Facet;
use serde::{Deserialize, Serialize};

pub use links_proto::{NodeKind, NodeRef};

/// What kind of collection this is. Named variants for the common cases;
/// [`CollectionKind::Other`] is the open escape hatch for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum CollectionKind {
    /// A song library — the master catalogue of works.
    Library,
    /// An ordered setlist — the songs for one service / gig, in order.
    Setlist,
    /// A show — a run of setlists / segments.
    Show,
    /// A generic playlist.
    Playlist,
    /// Anything else — the `String` is a free-form kind label.
    Other(String),
}

impl CollectionKind {
    /// A stable string form for storage / display. [`CollectionKind::Other`]
    /// renders its inner label verbatim.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Library => "library",
            Self::Setlist => "setlist",
            Self::Show => "show",
            Self::Playlist => "playlist",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// One entry in a [`Collection`]: a reference to a node plus its sort key.
///
/// `rank` is a lexorank fractional-index string; items in a collection are
/// kept sorted by it. Two items never share a rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct CollectionItem {
    /// The node this entry points at (a song, note, video, …).
    pub node: NodeRef,
    /// Lexorank sort key. Items are ordered ascending by this string.
    pub rank: String,
}

impl CollectionItem {
    #[must_use]
    pub fn new(node: NodeRef, rank: impl Into<String>) -> Self {
        Self {
            node,
            rank: rank.into(),
        }
    }
}

/// An ordered collection of node references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct Collection {
    /// Stable id (assigned by the backend on create if empty).
    pub id: String,
    /// Owning org.
    pub org: String,
    /// Human title.
    pub title: String,
    /// What kind of collection this is.
    pub kind: CollectionKind,
    /// The items, kept sorted ascending by [`CollectionItem::rank`].
    #[serde(default)]
    pub items: Vec<CollectionItem>,
}

impl Collection {
    /// A fresh, empty collection (no id yet — the backend assigns one).
    #[must_use]
    pub fn new(org: impl Into<String>, title: impl Into<String>, kind: CollectionKind) -> Self {
        Self {
            id: String::new(),
            org: org.into(),
            title: title.into(),
            kind,
            items: Vec::new(),
        }
    }

    /// Sort items ascending by rank (the canonical order). Called by the
    /// store after every mutation so readers see a sorted list.
    pub fn sort_items(&mut self) {
        self.items.sort_by(|a, b| a.rank.cmp(&b.rank));
    }
}
