//! Deep-Research flow. The LLM proposes search queries
//! grounded in a [`crate::graph::KnowledgeGap`] or
//! [`crate::review::ReviewItem`], the agent executes them
//! externally (Tavily / `SerpApi` / `SearXNG` / manual), and
//! submits results back as raw sources to be re-ingested.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "wiki_research_plans", repo)]
pub struct ResearchPlan {
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    /// What this plan targets. Either a knowledge-graph gap
    /// id, a lint finding id, or a review item id — backend
    /// disambiguates via `source_kind`.
    #[architect(json)]
    pub source_kind: ResearchSourceKind,
    #[architect(filterable)]
    pub source_id: String,
    /// Curator-facing topic statement. Editable before the
    /// plan executes.
    #[architect(filterable, fulltext)]
    pub topic: String,
    /// Search queries to run. Multi-query so the LLM can
    /// triangulate.
    #[architect(json)]
    pub queries: ResearchQueryList,
    #[architect(filterable, sortable)]
    pub created_at: DateTime<Utc>,
    #[architect(json, filterable)]
    pub status: ResearchStatus,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(C)]
pub enum ResearchSourceKind {
    KnowledgeGap,
    LintFinding,
    ReviewItem,
    /// Curator opened a free-form research session.
    #[default]
    Manual,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct ResearchQueryList(pub Vec<ResearchQuery>);

impl ResearchQueryList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<ResearchQuery>> for ResearchQueryList {
    fn from(v: Vec<ResearchQuery>) -> Self {
        Self(v)
    }
}

impl FromIterator<ResearchQuery> for ResearchQueryList {
    fn from_iter<I: IntoIterator<Item = ResearchQuery>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for ResearchQueryList {
    type Target = Vec<ResearchQuery>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ResearchQueryList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct ResearchQuery {
    /// Search engine to use (`"tavily"`, `"serpapi"`,
    /// `"searxng"`, `"manual"`).
    pub engine: String,
    /// Verbatim query string.
    pub q: String,
    /// Result cap.
    pub limit: u32,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(C)]
pub enum ResearchStatus {
    /// Plan generated but not yet started.
    #[default]
    Proposed,
    /// Agent is running queries.
    Running,
    /// All queries returned; awaiting [`crate::service::WikiService::submit_research_result`].
    Awaiting,
    /// Results submitted; spawned ingest tasks are tracked
    /// independently.
    Submitted,
    /// Curator cancelled before completion.
    Cancelled,
}

/// One result returned from a research query. Submitting a
/// batch of these spawns an [`crate::ingest::IngestTask`] per
/// row.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct RawSource {
    /// Where the source came from (URL, file path, etc.).
    pub origin: String,
    /// Filename to use when saving to `Wiki/raw/sources/`. The
    /// backend ensures uniqueness.
    pub filename: String,
    /// Source bytes (markdown, HTML, PDF, etc.). The
    /// backend persists this to `Wiki/raw/sources/<filename>`.
    pub bytes: Vec<u8>,
    /// MIME type.
    pub mime: String,
    /// Optional title from the search result. Used in
    /// `log.md` and the agent's analysis prompt.
    pub title: String,
}
