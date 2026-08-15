//! `GoalSession` — the autonomous-loop state for a `WorkSession`
//! driven by `task agent goal`.
//!
//! A coding `WorkSession` carries the state machine + audit trail;
//! a *goal* loop layers a completion condition and a turn budget on
//! top. This row persists the bits the loop needs to be inspected
//! (`goal status`), parked (`goal pause`), and resumed without losing
//! the directive (`goal resume` resets the counter and continues):
//!
//! - `condition` — the directive the evaluator judges against.
//! - `budget` — the turn ceiling (`--max-iters`).
//! - `turns_used` — turns consumed so far (reset on resume).
//! - `last_reason` — the evaluator's most recent "not met" reason,
//!   fed back into the next worker turn.
//!
//! One row per session, keyed by `session_id`. Stored alongside the
//! other workflow tables (`goals.json`) — see `WorkflowStore`.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Newtype around the list of subgoal strings — extra acceptance
/// criteria appended mid-run via `goal subgoal`. Wrapped so we can
/// derive `architect::JsonField` on it (orphan rules forbid
/// implementing the conversions for a bare `Vec<String>`), letting it
/// persist as a JSON column. Mirrors `Labels` in `agent-proto`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Facet, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq,
)]
#[repr(transparent)]
pub struct Subgoals(pub Vec<String>);

impl From<Vec<String>> for Subgoals {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl From<Subgoals> for Vec<String> {
    fn from(v: Subgoals) -> Self {
        v.0
    }
}

/// The goal-loop state attached to one [`WorkSession`](crate::WorkSession).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "goal_sessions", repo)]
pub struct GoalSession {
    /// FK + primary key: the session this goal drives. One goal per
    /// session, so the session id is the natural key.
    #[architect(primary_key, auto_increment = false)]
    pub session_id: Uuid,

    /// The completion condition the evaluator judges against.
    pub condition: String,

    /// Turn ceiling (`--max-iters`) before the session parks.
    pub budget: u32,

    /// Turns consumed so far. Reset to `0` on `goal resume`.
    #[serde(default)]
    pub turns_used: u32,

    /// The evaluator's latest "not met" reason, carried into the
    /// next worker turn. Empty before the first evaluation.
    #[serde(default)]
    pub last_reason: String,

    /// Extra acceptance criteria appended mid-run via `goal subgoal`.
    /// They are folded into the worker prompt and judged alongside the
    /// `condition`. Cleared whenever a fresh goal is set (`goal run`).
    #[serde(default)]
    #[architect(json)]
    pub subgoals: Subgoals,

    /// The worker's most recent observed step this run — e.g.
    /// `Edit apps/cli/src/main.rs` or `Bash: cargo build`, parsed from
    /// a `--output-format stream-json` worker. Empty for opaque
    /// workers. Surfaced by `goal status` so you can see what the loop
    /// is doing right now, not just that it's busy.
    #[serde(default)]
    pub current_activity: String,

    #[architect(filterable, sortable)]
    pub updated_at: DateTime<Utc>,
}

impl GoalSession {
    /// A fresh goal at turn 0 with no evaluator reason yet.
    #[must_use]
    pub fn new(session_id: Uuid, condition: impl Into<String>, budget: u32) -> Self {
        Self {
            session_id,
            condition: condition.into(),
            budget,
            turns_used: 0,
            last_reason: String::new(),
            subgoals: Subgoals::default(),
            current_activity: String::new(),
            updated_at: Utc::now(),
        }
    }
}
