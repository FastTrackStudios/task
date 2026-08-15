//! Async review queue. The LLM flags items for human
//! judgment; the curator works the queue at their pace.
//! Persisted to `Wiki/_state/review.json`.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct ReviewItem {
    pub id: String,
    pub kind: ReviewKind,
    /// Pages the item concerns.
    pub subjects: Vec<String>,
    /// Curator-facing description.
    pub message: String,
    /// Pre-generated suggestions the curator can choose from.
    /// Each becomes the body of a [`ReviewAction::Apply`].
    pub suggestions: Vec<ReviewSuggestion>,
    pub raised_at: DateTime<Utc>,
    pub status: ReviewStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum ReviewKind {
    /// Two pages make conflicting claims. Curator decides
    /// which is correct.
    Contradiction,
    /// A new ingest wants to overwrite a page significantly.
    /// Curator approves or rewrites the diff.
    DestructiveEdit,
    /// LLM is uncertain about a claim and wants curator
    /// confirmation before filing.
    Uncertain,
    /// Schema violation that the LLM couldn't auto-fix.
    SchemaConflict,
    /// Federation: a peer wiki's page has an updated version
    /// of a page we also maintain. Curator chooses merge
    /// strategy.
    PeerDivergence,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct ReviewSuggestion {
    pub label: String,
    /// What this option would change. Backends apply it
    /// verbatim if the curator picks it.
    pub action: ReviewAction,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub enum ReviewAction {
    /// Replace a page's body with new markdown.
    RewritePage { path: String, markdown: String },
    /// Append a note to a page (under a `## Edit log` section
    /// or similar; backend decides the convention).
    AppendNote { path: String, body: String },
    /// Run a web search to gather more sources before
    /// deciding. Converts to a [`crate::research::ResearchPlan`].
    Research { query: String },
    /// Curator accepted the item without on-disk changes.
    AcceptNoOp,
    /// Curator rejected — don't raise again.
    Dismiss { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum ReviewStatus {
    Open,
    Resolved,
    Dismissed,
}
