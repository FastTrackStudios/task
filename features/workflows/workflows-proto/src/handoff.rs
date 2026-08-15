//! `Handoff` — a parked session's "where I left off" note.
//!
//! When an agent runs out of context, hits a blocker it can't
//! resolve, or just finishes the chunk it can do, it posts a
//! `Handoff`. A subsequent `resume` reads the handoff so the
//! continuing actor sees the previous one's summary +
//! open questions without re-grepping the codebase.
//!
//! Multiple handoffs per session are allowed (a session can be
//! parked, resumed, parked again). The `status` enum gates
//! whether a handoff is still actionable.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct HandoffId(pub Uuid);

impl HandoffId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for HandoffId {
    fn default() -> Self {
        Self::new()
    }
}

/// Why this handoff exists. Used by `task code inbox` to sort
/// — `NeedsHumanInput` surfaces ahead of `ContextLimit`, etc.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[repr(u8)]
pub enum HandoffReason {
    /// A human needs to answer a question / make a call.
    NeedsHumanInput,
    /// External system is blocking (CI red, dependency missing, API rate-limited).
    BlockedExternal,
    /// The agent ran out of usable context.
    ContextLimit,
    /// The work needed turned out to be out of the current
    /// session's scope; another session should handle it.
    OutOfScope,
    /// Natural end-of-shift; no specific blocker.
    EndOfChunk,
    /// Generic escape hatch. `tag` names the reason.
    Custom { tag: String },
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum HandoffStatus {
    /// Waiting for someone to claim/resume.
    Open,
    /// An actor has called `resume` and is now working on it.
    Claimed,
    /// Marked resolved — work continued and the handoff's
    /// concern is addressed.
    Resolved,
    /// Withdrawn; no longer relevant.
    Cancelled,
}

/// One handoff per pause point.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "workflow_handoffs", repo)]
pub struct Handoff {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::now_v7())]
    pub id: Uuid,

    /// FK to `WorkSession::id`.
    #[architect(filterable, sortable)]
    pub session_id: Uuid,

    /// Who's parking the work.
    #[architect(json)]
    pub from_actor: AgentRef,

    /// Specific successor, or `None` for "anyone can claim".
    /// `task code inbox --as-agent X` returns:
    ///   - all handoffs where `to_actor = Some(X)`
    ///   - all handoffs where `to_actor = None` that X is
    ///     allowed to take.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(json)]
    pub to_actor: Option<AgentRef>,

    #[architect(json)]
    pub reason: HandoffReason,

    #[architect(json)]
    pub status: HandoffStatus,

    /// Markdown — what the previous actor did, in their voice.
    /// Mostly auto-drafted from recent `Activity` rows; the
    /// agent can edit before posting.
    pub summary: String,

    /// Bullet list, markdown. Specific things the continuing
    /// actor needs to answer / decide.
    #[serde(default)]
    pub open_questions: String,

    /// Bullet list, markdown. Concrete next steps the previous
    /// actor would have taken next if they'd had budget.
    #[serde(default)]
    pub recommended_next: String,

    #[architect(filterable, sortable)]
    pub created_at: DateTime<Utc>,

    /// Set when `status` flips to `Resolved` / `Cancelled`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resolved_at: Option<DateTime<Utc>>,
}

impl Handoff {
    /// Build an `Open` handoff at "now".
    #[must_use]
    pub fn post(
        session_id: Uuid,
        from_actor: AgentRef,
        reason: HandoffReason,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            session_id,
            from_actor,
            to_actor: None,
            reason,
            status: HandoffStatus::Open,
            summary: summary.into(),
            open_questions: String::new(),
            recommended_next: String::new(),
            created_at: Utc::now(),
            resolved_at: None,
        }
    }
}
