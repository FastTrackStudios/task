//! Wiki health checks. Some are computable from the vault
//! alone (orphan, stale, missing-page); others need LLM
//! judgment (contradiction, missing-cross-ref). The trait
//! exposes both flavors the same way — the backend decides
//! which scope is cheap-and-local vs. requires an agent.

use chrono::{DateTime, Utc};
use facet::Facet;

/// What to check. `All` runs every scope; the others are
/// targeted (cheap iteration when the curator is fixing a
/// specific class of issue).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum LintScope {
    All,
    /// Pages with degree ≤ 1 (no inbound or outbound links
    /// other than themselves). Cheap, no LLM needed.
    Orphan,
    /// Pages whose newest cited source is older than N days
    /// (configurable per-wiki in `schema.md`).
    Stale,
    /// Pages that mention `[[X]]` where `X.md` doesn't exist.
    /// Cheap, no LLM.
    MissingPage,
    /// Pages A and B share sources or topics but don't link.
    /// Requires LLM to judge whether they *should*.
    MissingCrossRef,
    /// Pages making conflicting claims. LLM-only.
    Contradiction,
    /// Frontmatter validation — required fields missing,
    /// types unknown. Cheap, no LLM.
    Schema,
}

/// One lint hit. Persisted to
/// `Wiki/_state/lint_findings.json` so findings survive
/// restarts and can be resolved over multiple sessions.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct LintFinding {
    pub id: String,
    pub scope: LintScope,
    /// Pages involved. Single entry for orphan / stale /
    /// schema; multiple for cross-ref / contradiction.
    pub subjects: Vec<String>,
    /// Human-facing description.
    pub message: String,
    /// Severity bucket — backends sort by this in the
    /// findings list.
    pub severity: Severity,
    /// Free-form suggestion (LLM-authored or rule-based)
    /// for how to resolve.
    pub suggestion: String,
    /// When this finding was raised.
    pub raised_at: DateTime<Utc>,
    /// Resolution state.
    pub status: FindingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum Severity {
    /// Worth surfacing on the next lint pass but no rush.
    Info,
    /// Should be fixed soon — wiki quality is degrading.
    Warn,
    /// Wiki is internally inconsistent. Fix or explicitly
    /// dismiss.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum FindingStatus {
    Open,
    Resolved,
    /// Curator decided this is acceptable; don't raise again.
    Dismissed,
}

/// Action the curator (or agent) takes on a finding.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub enum FindingAction {
    /// Mark resolved without changes (e.g. the finding was
    /// fixed out-of-band).
    Resolve,
    /// Dismiss permanently — schema rules should be tweaked
    /// instead.
    Dismiss { reason: String },
    /// Convert into a [`crate::review::ReviewItem`] for
    /// async curator judgment.
    PromoteToReview,
    /// Convert into a [`crate::research::ResearchPlan`] —
    /// finding implies we need more sources.
    PromoteToResearch,
}
