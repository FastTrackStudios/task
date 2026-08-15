//! `PageMeta` / `FolderIndex` — lightweight, wasm-clean
//! per-page metadata for the virtual-folder sidebar.
//!
//! The sidebar organizes the vault by each note's `folder`
//! frontmatter property (an Obsidian "folder note" wikilink,
//! e.g. `folder: "[[Wisdom]]"`), not by physical directories.
//! Building that tree needs each file's frontmatter, but the
//! [`crate::Manifest`] only carries path/sha/mtime — and a wasm
//! client can't afford to fetch + parse every file. So the
//! server parses frontmatter once and returns this slim index;
//! the client builds the tree from it.
//!
//! Pure data (no `SystemTime`/`PathBuf`) so it round-trips on
//! `wasm32`, same as [`crate::VaultPage`].

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Frontmatter-derived metadata for one `.md` page.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct PageMeta {
    /// Vault-relative path, forward-slash separated. Stable
    /// identity; what `get_file` / `put_file` / `set_folder`
    /// address.
    pub path: String,
    /// Filename without extension — the wikilink target other
    /// notes point at via `folder: [[basename]]`, and the
    /// dedup/tree key.
    pub basename: String,
    /// Frontmatter `title`, or the basename when absent.
    pub title: String,
    /// Frontmatter `type` (`""`, `"folder"`, `"index"`, …).
    /// Folder-ness is also inferred from having children, but
    /// this lets an empty folder note still read as a folder.
    pub page_type: String,
    /// Parent folder note's basename, parsed from the `folder`
    /// frontmatter wikilink (`[[X]]` → `X`). Empty string means
    /// "no parent" → a tree root.
    pub folder: String,
    /// Frontmatter tags (`tags:` / `tag:`), `#` stripped, document
    /// order. Hierarchical tags keep their `/` (e.g. `ops/inventory`)
    /// — the client's tag-folder explorer splits on it.
    pub tags: Vec<String>,
    /// Frontmatter `icon` — a curated lucide icon name (kebab-case,
    /// e.g. `heart-pulse`). Tag notes (`type: tag`) use it to give a
    /// tag a sidebar icon; empty when unset.
    pub icon: String,
    /// Content hash (lowercase hex sha256), matching the
    /// manifest's hashing — lets the client issue a conditional
    /// `set_folder` / open without a separate round-trip.
    pub sha256: String,
    /// Frontmatter `aliases` (and singular `alias`) values —
    /// alternate wikilink names for the page. Feeds the editor's
    /// `[[…]]` autocomplete candidates without another RPC.
    /// Empty for pages without aliases.
    pub aliases: Vec<String>,
}

/// Full folder index for one vault. `vault_id` echoes the
/// request; `pages` is unordered (the client builds + sorts the
/// tree).
#[derive(Debug, Clone, Facet)]
pub struct FolderIndex {
    pub vault_id: String,
    pub pages: Vec<PageMeta>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{FolderIndex, PageMeta};
    unsafe impl vox_types::Reborrow for PageMeta {
        type Ref<'a> = PageMeta;
    }
    unsafe impl vox_types::Reborrow for FolderIndex {
        type Ref<'a> = FolderIndex;
    }
}
