//! Index + log — the catalog and timeline.
//!
//! Both are markdown files under `Wiki/`, but this crate
//! exposes them as structured types so backends can produce
//! consistent output without re-parsing markdown every time.
//! Convert with the backend's `serialize_index` /
//! `parse_index` helpers (those live in `wiki-live`, not here).

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Parsed view of `Wiki/index.md`. The on-disk markdown is the
/// source of truth; this is the LLM-readable mirror.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct WikiIndex {
    /// One section per page `type:` (`entity`, `concept`, etc.).
    /// Sections are emitted in stable order so diffs are
    /// reviewable.
    pub sections: Vec<IndexSection>,
    /// Total pages indexed. Convenience; computable from
    /// `sections`.
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct IndexSection {
    /// Page `type:` this section covers (e.g. `"concept"`).
    pub page_type: String,
    /// Entries sorted by title.
    pub entries: Vec<IndexEntry>,
}

/// Catalog row for one wiki page. PK is the vault-relative
/// `path` (natural key — a single file lives at one path). The
/// `WikiIndex` aggregate stays a wire wrapper; the SQLite mirror
/// derived from this entity is what powers Phase 3 federated
/// FTS5 lookups.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "wiki_index_entries", repo)]
pub struct IndexEntry {
    /// Vault-relative path (e.g. `Wiki/Concepts/Foo.md`). PK.
    #[architect(primary_key, auto_increment = false)]
    pub path: String,
    /// Page title, used as the wikilink target.
    #[architect(filterable, sortable, fulltext)]
    pub title: String,
    /// Page `type:` from frontmatter (`entity`, `concept`, …).
    /// Stored on the row so section aggregation is an index hit.
    #[architect(filterable, sortable)]
    #[serde(default)]
    pub page_type: String,
    /// One-line summary lifted from the page's first paragraph
    /// or the LLM's analysis. May be empty.
    #[architect(fulltext)]
    pub summary: String,
    /// Count of `sources:` entries this page cites (for
    /// `source`-typed pages, always 1).
    #[architect(filterable, sortable)]
    pub source_count: u32,
}

/// One entry appended to `Wiki/log.md`. Backends serialize
/// these as a fenced markdown section starting `## [YYYY-MM-DD]
/// <op> | <title>` so the timeline stays grep-able.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "wiki_log_entries", repo)]
pub struct LogEntry {
    /// Synthetic id. Markdown stays the source of truth; the
    /// PK lets the SQLite mirror dedupe replays.
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,
    /// When this happened. Wall-clock UTC; the serializer
    /// renders the date in the header and the full timestamp
    /// in the body.
    #[architect(filterable, sortable)]
    pub at: DateTime<Utc>,
    /// The operation kind.
    #[architect(json, filterable)]
    pub op: LogOp,
    /// Short human-facing title (page title for ingest, query
    /// text for query, scope name for lint, etc.).
    #[architect(filterable, fulltext)]
    pub title: String,
    /// Optional free-form body. Markdown; rendered under the
    /// section header.
    #[architect(fulltext)]
    pub body: String,
    /// Pages touched by this operation, if any. Wikilink-able
    /// titles.
    #[architect(json)]
    pub pages_touched: WikilinkList,
}

/// `Vec<String>` newtype for [`LogEntry::pages_touched`].
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct WikilinkList(pub Vec<String>);

impl WikilinkList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for WikilinkList {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for WikilinkList {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for WikilinkList {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for WikilinkList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(C)]
pub enum LogOp {
    Ingest,
    Query,
    Lint,
    Review,
    Research,
    /// Bootstrap (wiki created, schema written, etc.) and
    /// other one-off admin events.
    #[default]
    Admin,
}

/// A clock-free date used in the log header (`[2026-05-21]`).
/// Wraps `chrono::NaiveDate` to keep the wire shape obvious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct LogDate {
    pub date: NaiveDate,
}
