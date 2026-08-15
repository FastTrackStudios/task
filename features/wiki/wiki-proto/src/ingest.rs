//! Two-step chain-of-thought ingest pipeline (LLM-agnostic).
//!
//! Mirrors `llm_wiki`'s `claude_cli.rs` + `file_sync.rs` queue
//! shape. The trait methods accept the LLM's output as data —
//! the LLM glue itself (claude CLI / Anthropic API / Hermes)
//! lives outside this crate. Any agent can drive the pipeline
//! by:
//!
//! 1. `enqueue_ingest(source)` — register a new raw source.
//! 2. `claim_next_ingest()` — pop one pending task.
//! 3. Agent reads source + `schema.md` + `purpose.md` +
//!    `index.md`.
//! 4. `record_analysis(task_id, analysis)` — store the
//!    step-1 chain-of-thought output.
//! 5. Agent generates pages.
//! 6. `record_pages(task_id, pages)` — backend writes them
//!    to disk, updates `index.md`, appends to `log.md`.
//! 7. `complete_ingest(task_id)` — flip state to `Done`.
//!
//! State transitions enforced by the backend; calling out of
//! order ⇒ [`crate::error::WikiError::IllegalState`].

use chrono::{DateTime, Utc};
use facet::Facet;

/// One task in the ingest queue. Backends persist these to
/// `Wiki/_state/ingest_queue.json` so crashes don't lose
/// in-flight work.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct IngestTask {
    /// Stable id (e.g. uuid). Persists across restarts.
    pub id: String,
    /// Vault-relative path to the raw source file.
    pub source_path: String,
    /// Change kind that triggered the task.
    pub kind: SourceChange,
    /// Current state in the pipeline.
    pub status: IngestStatus,
    /// SHA-256 of the source bytes at enqueue time. Used by
    /// the backend to skip re-ingest of unchanged files.
    pub source_sha256: String,
    /// Step-1 chain-of-thought output, once recorded.
    pub analysis: Option<AnalysisDraft>,
    /// Step-2 generated pages, once recorded. Empty until
    /// `record_pages` lands.
    pub pages: Vec<PageDraft>,
    /// Retry count. Backends auto-retry transient failures
    /// up to a cap (default 3, matches `llm_wiki`).
    pub retries: u32,
    /// Last failure message, if any.
    pub last_error: Option<String>,
    /// Wall-clock timestamps for telemetry.
    pub enqueued_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SourceChange {
    /// New file landed in `Wiki/raw/sources/`.
    Created,
    /// Existing source content changed (sha256 diff).
    Modified,
    /// Source removed — wiki should reconcile by dropping
    /// orphaned `source:`-typed pages and pruning citations.
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum IngestStatus {
    /// Newly enqueued, no work started.
    Pending,
    /// An agent claimed it; analysis in progress.
    Analyzing,
    /// `record_analysis` landed; waiting on page generation.
    Generating,
    /// `record_pages` landed; backend is writing pages /
    /// updating index + log.
    Writing,
    /// All steps completed cleanly.
    Done,
    /// Hit `retries == max_retries` with a recorded error.
    Failed,
    /// Curator cancelled it.
    Cancelled,
}

/// Step-1 output: the LLM's analysis of the source before any
/// pages are generated. Backends store this so step 2 can be
/// retried without re-running analysis.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct AnalysisDraft {
    /// Free-form markdown — the LLM's chain-of-thought.
    /// Surfaced to the curator in case they want to steer
    /// page generation manually.
    pub notes: String,
    /// Entities the agent identified (people, orgs,
    /// products). Each maps to a candidate `entity`-typed
    /// page.
    pub entities: Vec<EntityHint>,
    /// Concepts the agent identified. Each maps to a
    /// candidate `concept`-typed page.
    pub concepts: Vec<ConceptHint>,
    /// Existing wiki pages the agent thinks need updates,
    /// keyed by vault-relative path. Value = short reason.
    pub pages_to_update: Vec<PageUpdateHint>,
    /// Contradictions the agent flagged. Each becomes a
    /// [`crate::review::ReviewItem`] of kind `Contradiction`.
    pub contradictions: Vec<ContradictionHint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct EntityHint {
    pub name: String,
    /// One-line summary the agent will use as the page's
    /// opening paragraph.
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ConceptHint {
    pub name: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct PageUpdateHint {
    /// Vault-relative path to the existing page.
    pub path: String,
    /// Why this update is needed (LLM-authored).
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ContradictionHint {
    /// Pages involved in the contradiction.
    pub pages: Vec<String>,
    /// LLM's description of the conflict.
    pub description: String,
}

/// Step-2 output: a single page the agent wants to write.
/// Backends apply these atomically — if any write fails, the
/// whole batch is rolled back.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct PageDraft {
    /// Vault-relative target path. `Wiki/<Type>/<Slug>.md` by
    /// convention but the backend doesn't enforce.
    pub path: String,
    /// Full markdown source including YAML frontmatter.
    pub markdown: String,
    /// Whether this overwrites an existing page (`true`) or
    /// creates a new one (`false`). Backends use this to
    /// validate against the on-disk state.
    pub overwrite: bool,
}
