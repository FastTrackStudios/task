//! `WorkSession` — one instance of an actor working on a
//! subject inside a workflow.
//!
//! Sessions are open while work is in progress, parked when an
//! agent pauses with a [`Handoff`](crate::Handoff), and closed
//! on `Finished` / `Cancelled`. A subject (e.g. a TaskInfo)
//! has at most one `Active` session at a time across the org;
//! handoff + resume swaps which actor holds it.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentRef;

/// Stable per-session identifier. UUID v7 so insertion-order
/// is recoverable from the id (handy for time-sorted listings
/// without an extra index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which workflow this session belongs to. Open-ended so new
/// domains (music, plugin-decode, …) can be added without a
/// schema change.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[repr(u8)]
pub enum WorkflowKind {
    /// `task code *` — the coding agent dev loop.
    Coding,
    /// Wiki ingest / review / deepen.
    Writing,
    /// Keyflow / DAW work.
    Music,
    /// Wiki research-plan execution.
    Research,
    /// AAX/VST3 reverse engineering, see the `decode-plugin` skill.
    Plugin,
    /// Escape hatch for ad-hoc workflows. `tag` names the kind
    /// so reports can still group.
    Custom { tag: String },
}

/// What's being worked on. Each workflow has a primary subject
/// type — coding works on a TaskInfo, writing on a wiki page,
/// music on a chart. The discriminator keeps the audit log
/// typed without forcing a generic on every consumer.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[repr(u8)]
pub enum SubjectRef {
    /// `TaskInfo::id`. The canonical "issue" in Task.
    Task { id: Uuid },
    /// Vault-relative path to a wiki page.
    WikiPage { path: String },
    /// Vault-relative path to a `.kf` chart.
    KeyflowChart { path: String },
    /// Vault-relative path to a session's recovered DAW project.
    DawProject { path: String },
    /// Generic escape hatch for unmodelled subjects. `tag` names
    /// the subject kind (renamed from `kind` to avoid colliding
    /// with serde's `tag = "kind"` discriminator).
    Custom { tag: String, id: String },
}

/// Lifecycle status of a session.
///
/// Transition rules (enforced by the orchestrator, not this crate):
///
/// - `Active` ⇄ `Parked` (via `park` / `resume`)
/// - `Active` → `Blocked` → `Active` (external blocker resolved)
/// - `Active` | `Parked` | `Blocked` → `Finished` (normal end)
/// - `Active` | `Parked` | `Blocked` → `Cancelled` (abandon)
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum SessionStatus {
    Active,
    Parked,
    Blocked,
    Finished,
    Cancelled,
}

/// One instance of work on a subject by an actor.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "work_sessions", repo)]
pub struct WorkSession {
    /// Stable id. UUIDv7 so listings sort chronologically.
    #[architect(primary_key, auto_increment = false, on_create = Uuid::now_v7())]
    pub id: Uuid,

    /// Which workflow this session is in.
    #[architect(json)]
    pub workflow: WorkflowKind,

    /// What's being worked on.
    #[architect(json)]
    pub subject: SubjectRef,

    /// Who started the session. May differ from later actors
    /// after a handoff/resume — see [`Activity`] for the
    /// per-event attribution.
    #[architect(json)]
    pub started_by: AgentRef,

    /// Current owner — set on every resume.
    #[architect(json)]
    pub current_actor: AgentRef,

    #[architect(json)]
    pub status: SessionStatus,

    #[architect(filterable, sortable)]
    pub started_at: DateTime<Utc>,

    #[architect(filterable, sortable)]
    pub updated_at: DateTime<Utc>,

    /// Set when status transitions out of `Active`/`Parked`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ended_at: Option<DateTime<Utc>>,

    /// Free-form notes the actor jots while working. Survives
    /// park/resume so the next agent sees the previous one's
    /// scribbles. Markdown encouraged.
    #[serde(default)]
    pub scratchpad: String,
}

impl WorkSession {
    /// Start a fresh session in `Active` state.
    #[must_use]
    pub fn start(workflow: WorkflowKind, subject: SubjectRef, actor: AgentRef) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            workflow,
            subject,
            started_by: actor.clone(),
            current_actor: actor,
            status: SessionStatus::Active,
            started_at: now,
            updated_at: now,
            ended_at: None,
            scratchpad: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_time_ordered() {
        let a = SessionId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = SessionId::new();
        // UUIDv7 timestamp prefix: byte-wise comparison reflects creation order.
        assert!(a.0.as_bytes() < b.0.as_bytes());
    }

    #[test]
    fn start_session_initializes_state() {
        let s = WorkSession::start(
            WorkflowKind::Coding,
            SubjectRef::Task { id: Uuid::new_v4() },
            AgentRef::agent("claude"),
        );
        assert_eq!(s.status, SessionStatus::Active);
        assert!(s.ended_at.is_none());
        assert_eq!(s.started_by, s.current_actor);
    }
}
