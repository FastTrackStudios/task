//! Raw layer — immutable user-imported sources.
//!
//! Mirrors `llm_wiki`'s `raw/sources/` split: anything under
//! `Wiki/raw/sources/` is **immutable from the wiki's perspective**.
//! The agent reads from it, cites it via the `sources:`
//! frontmatter field on generated pages, but never rewrites
//! the file bytes. Backends enforce this on `record_pages` —
//! drafts targeting paths under `Wiki/raw/sources/` are rejected
//! with [`crate::error::WikiError::IllegalState`].
//!
//! New raw bytes only land via [`crate::service::WikiService::import_raw_source`]
//! or [`crate::service::WikiService::submit_research_result`].
//! Removal goes through [`crate::service::WikiService::delete_raw_source`],
//! which returns review items for any orphaned `source`-typed
//! pages so the curator can decide whether to keep the
//! distilled write-up.

use chrono::{DateTime, Utc};
use facet::Facet;

/// What changed under `Wiki/raw/sources/` since the last
/// snapshot. Returned by
/// [`crate::service::raw_layer::RawLayer::rescan_diff`] — the
/// read-only sibling of `rescan_sources` (which also enqueues
/// ingest tasks for every created/modified entry). Paths are
/// wiki-root-relative (`raw/sources/…`).
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct SourceDiff {
    /// New files since the last snapshot.
    pub created: Vec<String>,
    /// Files whose bytes changed.
    pub modified: Vec<String>,
    /// Files that have disappeared from disk since the last
    /// scan.
    pub deleted: Vec<String>,
}

/// Arguments for [`crate::service::WikiService::import_raw_source`].
/// Grouped into a struct because the architect macro's
/// `Facet` requirement caps tuple args at 4.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ImportRawSource {
    /// Filename to use under `Wiki/raw/sources/`. Backend
    /// uniqueness-resolves collisions by suffixing.
    pub filename: String,
    /// MIME type.
    pub mime: String,
    /// Optional human title (used in `log.md` + analysis
    /// prompt).
    pub title: String,
    /// Raw bytes.
    pub bytes: Vec<u8>,
    /// If `true`, enqueue an ingest task as `Created` once
    /// the file lands so the agent picks it up.
    pub auto_enqueue: bool,
}

/// Pointer to one entry in `Wiki/raw/sources/`. Returned by
/// [`crate::service::WikiService::list_raw_sources`] and
/// [`crate::service::WikiService::import_raw_source`].
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct RawSourceRef {
    /// Vault-relative path (e.g. `Wiki/raw/sources/foo.pdf`).
    pub path: String,
    /// Filename without directory (`foo.pdf`).
    pub filename: String,
    /// MIME type recorded at import time.
    pub mime: String,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 of the bytes. Used by the snapshot mechanism to
    /// detect drift and to dedupe imports.
    pub sha256: String,
    /// Wall-clock time the file landed in the wiki.
    pub imported_at: DateTime<Utc>,
    /// Optional human title from the import. Used in
    /// `log.md` and the agent's analysis prompt.
    pub title: String,
}
