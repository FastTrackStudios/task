//! `Transition` — one state change in a session, with attribution.
//!
//! Every time a session moves from one workflow state to
//! another (e.g. `Branched → Pushed` in the coding workflow,
//! `Drafted → Reviewed` in the writing workflow) a row lands
//! here. The `from` and `to` columns are strings rather than
//! a typed enum because the state space is per-workflow —
//! `workflows-proto` is domain-agnostic.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct TransitionId(pub Uuid);

impl TransitionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for TransitionId {
    fn default() -> Self {
        Self::new()
    }
}

/// One row per state change.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "workflow_transitions", repo)]
pub struct Transition {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::now_v7())]
    pub id: Uuid,

    /// Which session this transition belongs to. FK to
    /// `WorkSession::id`; not enforced at the type level here
    /// to keep the proto crate dependency-free.
    #[architect(filterable, sortable)]
    pub session_id: Uuid,

    /// Domain-specific state name. e.g. `"branched"` for the
    /// coding workflow's `CodingState::Branched`.
    #[architect(filterable)]
    pub from_state: String,

    #[architect(filterable)]
    pub to_state: String,

    #[architect(json)]
    pub actor: AgentRef,

    #[architect(filterable, sortable)]
    pub at: DateTime<Utc>,

    /// Optional human-readable reason; mostly empty for routine
    /// transitions, set when an agent wants to leave a breadcrumb.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

impl Transition {
    /// Build a transition row at "now".
    #[must_use]
    pub fn record(
        session_id: Uuid,
        from_state: impl Into<String>,
        to_state: impl Into<String>,
        actor: AgentRef,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            session_id,
            from_state: from_state.into(),
            to_state: to_state.into(),
            actor,
            at: Utc::now(),
            note: None,
        }
    }
}
