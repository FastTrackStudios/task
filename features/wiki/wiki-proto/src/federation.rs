//! Multi-wiki federation. A vault's `Wiki/` can subscribe to
//! peer wikis (other people's vaults, public knowledge bases,
//! team wikis). Cross-wiki wikilinks resolve via the registered
//! peer set; pulled pages are mirrored under
//! `Wiki/raw/sources/<peer-id>/` and treated as raw sources for
//! ingest — the local LLM rewrites them in the local voice.
//!
//! Federation is an *optional* layer. A wiki with zero peers
//! works exactly the same as a single-vault wiki.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "wiki_peers", repo)]
pub struct PeerWiki {
    /// Stable id chosen by the curator (e.g. `"alice-pkm"`).
    /// Becomes the `Wiki/raw/sources/<peer-id>/` directory name
    /// + the prefix in federated wikilinks (`[[alice:Page]]`).
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    /// URL to the peer's wiki-proto endpoint
    /// (`https://alice.example/api/v1/wiki` or
    /// `task://<peer-id>` for in-fleet peers).
    #[architect(filterable)]
    pub url: String,
    /// Curator-facing label.
    #[architect(filterable, sortable)]
    pub label: String,
    /// Auth token sent in `Authorization: Bearer ...`. Empty
    /// for public peers.
    pub auth_token: String,
    /// Last successful pull. Used as the `since` cursor for
    /// incremental sync.
    #[architect(filterable, sortable)]
    pub last_pulled_at: Option<DateTime<Utc>>,
}

/// Result of a single pull. Backends fan these out into
/// per-page [`crate::ingest::IngestTask`] entries.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct PeerPullResult {
    pub peer_id: String,
    pub pulled_at: DateTime<Utc>,
    /// Pages whose content changed (or are new).
    pub changed: Vec<PeerPagePtr>,
    /// Pages the peer removed since `since`.
    pub removed: Vec<String>,
    /// If the peer reports its own conflicts with our state
    /// (e.g. we and the peer both have `Spaced repetition`
    /// but the bodies diverged), they become
    /// [`crate::review::ReviewItem`]s of kind
    /// `PeerDivergence`.
    pub divergences: Vec<PeerPagePtr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct PeerPagePtr {
    /// Peer-relative path.
    pub path: String,
    /// SHA-256 of the page bytes at pull time. Local
    /// snapshot uses this to detect drift on the next pull.
    pub sha256: String,
    /// Page title from the peer's frontmatter — used to
    /// route into review or ingest with a stable identity.
    pub title: String,
}

/// Reference to a single page that resolved via a peer
/// wikilink. Returned by
/// [`crate::service::WikiService::resolve_cross_wiki_link`].
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct CrossWikiPageRef {
    pub peer_id: String,
    pub path: String,
    pub title: String,
    /// Local mirror path under `Wiki/raw/sources/<peer-id>/` if
    /// we've already pulled this page. Empty if we haven't.
    pub local_mirror: String,
}
