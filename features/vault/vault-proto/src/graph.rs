//! `VaultGraph` — read-only wikilink-graph queries over a vault,
//! decorated with `#[architect::rpc]`.
//!
//! Server-side this is answered by `vault_obsidian::GraphBackend`
//! over the in-memory `LinkIndex` (see
//! `features/vault/vault-obsidian/src/links.rs`); the web client
//! reaches the same surface via the auto-emitted
//! [`VaultGraphClient`](crate::VaultGraphClient). It exists as a
//! **separate trait** from [`crate::VaultSync`] on purpose: sync
//! is the file-replication contract, graph is a derived/read-only
//! view — keeping them apart lets a backend serve one without the
//! other.
//!
//! All paths are vault-relative, forward-slash separated —
//! the same identity [`crate::VaultSync::get_file`] addresses.
//!
//! CLI parity is intentionally absent: `task vault
//! backlinks/links/orphans/...` already answer these queries
//! FS-natively. This trait is the web client's path.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::VaultSyncError;

/// One outgoing wikilink edge from a page. Mirrors
/// `vault_obsidian::OutgoingLink` with owned strings for the wire.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct GraphLink {
    /// The raw link target as written (`Page`, `Page#Heading`, …),
    /// anchor included.
    pub linkpath: String,
    /// Vault-relative path of the resolved target page, or `None`
    /// when the link doesn't resolve.
    pub resolved: Option<String>,
    /// Display alias (`[[Page|alias]]`), when present.
    pub alias: Option<String>,
}

/// One unresolved wikilink occurrence: `source` page contains a
/// link whose `linkpath` resolves to nothing.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct GraphUnresolved {
    /// Vault-relative path of the page containing the link.
    pub source: String,
    /// The raw link target that failed to resolve.
    pub linkpath: String,
}

/// One vault tag + how many pages carry it. Tags are normalized
/// without the leading `#`; nested tags keep their full
/// `parent/child` path.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct TagCount {
    pub tag: String,
    /// Number of pages carrying the tag (frontmatter `tags:` or
    /// inline `#tag`), each page counted once.
    pub count: u64,
}

/// Link-graph queries over one vault. Read-only; every call
/// reflects the vault's state at call time (the backend rebuilds
/// its index from disk per request, same freshness contract as
/// [`crate::VaultSync::folder_index`]).
#[architect::rpc]
pub trait VaultGraph {
    /// Pages linking TO `path`. Sorted by rel_path. Unknown
    /// `path` yields an empty list, not an error (a brand-new
    /// page legitimately has no backlinks).
    fn backlinks(&self, vault_id: &str, path: &str) -> Result<Vec<String>, VaultSyncError>;

    /// Outgoing wikilinks from `path`, de-duped in source order.
    fn links(&self, vault_id: &str, path: &str) -> Result<Vec<GraphLink>, VaultSyncError>;

    /// Pages with zero incoming links. Sorted.
    fn orphans(&self, vault_id: &str) -> Result<Vec<String>, VaultSyncError>;

    /// All `(source, linkpath)` pairs across the vault whose
    /// target resolves to nothing.
    fn unresolved(&self, vault_id: &str) -> Result<Vec<GraphUnresolved>, VaultSyncError>;

    /// Pages with zero outgoing links (body wikilinks, markdown
    /// links, and frontmatter wikilinks all count). Sorted.
    fn deadends(&self, vault_id: &str) -> Result<Vec<String>, VaultSyncError>;

    /// Every tag in the vault (frontmatter + inline `#tag`),
    /// with page counts, sorted by count desc then tag asc.
    /// Feeds the editor's tag-autocomplete candidates.
    fn tags(&self, vault_id: &str) -> Result<Vec<TagCount>, VaultSyncError>;
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{GraphLink, GraphUnresolved, TagCount};
    unsafe impl vox_types::Reborrow for GraphLink {
        type Ref<'a> = GraphLink;
    }
    unsafe impl vox_types::Reborrow for GraphUnresolved {
        type Ref<'a> = GraphUnresolved;
    }
    unsafe impl vox_types::Reborrow for TagCount {
        type Ref<'a> = TagCount;
    }
}
