//! Curated wiki pages — the markdown files under the wiki
//! root (`<Type>/<Slug>.md`, plus the catalog docs).
//!
//! This is the read/edit surface the wiki UI drives: the
//! knowledge-graph view resolves a node click to a
//! [`PageInfo::path`], opens it via
//! [`crate::service::Pages::read_page`], and saves edits with
//! the sha-guarded [`crate::service::Pages::write_page`].
//! Unlike the raw layer (immutable inputs), pages are the
//! curated, mutable layer — humans and agents both write here.

use chrono::{DateTime, Utc};
use facet::Facet;

/// Catalog row for one curated page. Cheap — no body.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct PageInfo {
    /// Wiki-root-relative path (e.g. `Concepts/Spaced repetition.md`).
    pub path: String,
    /// Frontmatter `title:`, falling back to the first `# heading`,
    /// then the file stem.
    pub title: String,
    /// Frontmatter `type:`; empty when absent.
    pub page_type: String,
    /// Size in bytes.
    pub size: u64,
    /// Filesystem mtime.
    pub modified: DateTime<Utc>,
    /// Frontmatter `ai_generated: true` — machine-produced content
    /// (AI summaries, ingests). The wiki is the home for material
    /// that isn't the user's own writing; this flag is the
    /// provenance badge distinguishing generated pages from curated
    /// human ones.
    pub ai_generated: bool,
    /// Frontmatter `generated_by:` — the model/agent that produced
    /// an `ai_generated` page; empty when absent.
    pub generated_by: String,
}

/// One page with its full markdown, plus the sha the editor
/// hands back to [`crate::service::Pages::write_page`] as its
/// optimistic-concurrency token.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct WikiPageDoc {
    /// Wiki-root-relative path.
    pub path: String,
    /// Full file contents, frontmatter included.
    pub markdown: String,
    /// SHA-256 of `markdown` as read from disk.
    pub sha256: String,
    /// Filesystem mtime.
    pub modified: DateTime<Utc>,
}
