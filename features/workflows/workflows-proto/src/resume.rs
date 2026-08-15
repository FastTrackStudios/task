//! Shared value types for the resume/handoff surface.
//!
//! These are the few domain-agnostic types worth defining up front
//! because *more than one* consumer touches them: the concrete
//! workflow (today, coding) returns a [`ResumeContext`], and the CLI
//! / future UI render it. Everything domain-specific — the state
//! machine, the orchestrator, the capability gate — lives with its
//! concrete workflow, not here.
//!
//! We deliberately do **not** define an abstract `Workflow` trait or
//! `Subject`/`State` marker traits yet. There's exactly one workflow
//! (coding); the right shared trait is the one extracted from two
//! real implementations, not guessed from one. Add it when a second
//! domain (writing, music, …) actually arrives.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::activity::Activity;
use crate::handoff::Handoff;
use crate::session::WorkSession;

/// The audit-log form of a workflow state: a plain string, because
/// [`Transition`](crate::Transition) is domain-agnostic. A concrete
/// state enum names itself (e.g. `"branched"`) and wraps it here for
/// type clarity at boundaries that don't know the enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransitionState(pub String);

impl TransitionState {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TransitionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TransitionState {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TransitionState {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// A pointer to something related to a session — surfaced in
/// [`ResumeContext`] so a resuming agent gets the neighbourhood, not
/// just the session row. Open-ended; the workflow's resume path
/// decides what to populate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelatedRef {
    /// A linked TaskInfo (blocker, parent, relates-to).
    Task { id: Uuid },
    /// A wiki page worth reading before resuming.
    WikiPage { path: String },
    /// A code symbol the work touches (`path` + symbol name).
    CodeSymbol { path: String, symbol: String },
    /// Any external resource (PR, ticket, doc).
    Url { href: String },
}

/// Everything a resuming agent needs to pick up where the last one
/// left off, assembled in one shot so the new actor doesn't have to
/// re-read the whole history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeContext {
    /// The session being resumed (now owned by the new actor).
    pub session: WorkSession,
    /// The state the session was last in, or `None` if it never
    /// transitioned past its start state.
    pub last_state: Option<TransitionState>,
    /// The most recent activity, newest first. Length is the
    /// assembler's choice (typically the last handful).
    pub recent_activity: Vec<Activity>,
    /// The handoff that parked this session, if it was parked.
    pub open_handoff: Option<Handoff>,
    /// Neighbouring context — related issues, pages, symbols.
    pub related: Vec<RelatedRef>,
    /// The previous actor's scratchpad, carried across verbatim.
    pub scratchpad: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_state_round_trips() {
        let ts = TransitionState::from("branched");
        assert_eq!(ts.as_str(), "branched");
        assert_eq!(ts.to_string(), "branched");
        let json = serde_json::to_string(&ts).unwrap();
        assert_eq!(json, "\"branched\"");
        let back: TransitionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ts);
    }

    #[test]
    fn related_ref_tags_its_kind() {
        let r = RelatedRef::CodeSymbol {
            path: "src/lib.rs".into(),
            symbol: "Orchestrator".into(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["kind"], "code_symbol");
        assert_eq!(json["symbol"], "Orchestrator");
    }
}
