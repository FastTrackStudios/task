//! `Activity` — non-state events in a session.
//!
//! Transitions cover state changes; activities cover everything
//! else — commits, comments, tool calls, file edits. The
//! payload is opaque JSON so workflows can record whatever
//! shape their events need without bloating this crate's
//! dependency surface.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct ActivityId(pub Uuid);

impl ActivityId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for ActivityId {
    fn default() -> Self {
        Self::new()
    }
}

/// Categorical bucket so reports can group + filter without
/// parsing the payload. Per-workflow event details live in
/// `Activity::payload`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[repr(u8)]
pub enum ActivityKind {
    /// A git commit was made on this session's branch.
    Commit,
    /// A comment was added to the subject (TaskInfo or forge issue).
    Comment,
    /// An external state change was observed (PR opened, CI passed, …).
    External,
    /// A capability-gated tool call was made (agent → external API).
    ToolCall,
    /// An agent posted a handoff. Mirrored as an `Activity` so the
    /// audit trail is dense — the `Handoff` row carries the detail.
    Handoff,
    /// Note attached to the scratchpad / changelog. Free-form.
    Note,
    /// Generic escape hatch. `tag` names the kind so reports still group.
    Custom { tag: String },
}

/// One row per event in a session.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "workflow_activities", repo)]
pub struct Activity {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::now_v7())]
    pub id: Uuid,

    /// FK to `WorkSession::id`.
    #[architect(filterable, sortable)]
    pub session_id: Uuid,

    /// Categorical bucket — see [`ActivityKind`].
    #[architect(json)]
    pub kind: ActivityKind,

    #[architect(json)]
    pub actor: AgentRef,

    #[architect(filterable, sortable)]
    pub at: DateTime<Utc>,

    /// Per-workflow event shape, JSON-encoded as a string.
    /// Stored as text (not a typed column) because
    /// `workflows-proto` is domain-agnostic and per-workflow
    /// schemas live in the consumer crates. Consumers parse
    /// according to the [`ActivityKind`]. Examples:
    ///
    /// - `Commit` → `{ "sha": "...", "message": "..." }`
    /// - `External` → `{ "kind": "ci_status", "state": "passed" }`
    /// - `ToolCall` → `{ "tool": "...", "args": {...}, "result": {...} }`
    ///
    /// Use [`Activity::set_payload`] / [`Activity::payload_as`]
    /// to round-trip through a typed shape.
    pub payload: String,
}

impl Activity {
    /// Build an activity row at "now". `payload` is any
    /// `Serialize` value; serialization failure is converted
    /// to the JSON `null` literal so the row always lands.
    #[must_use]
    pub fn record<P: serde::Serialize>(
        session_id: Uuid,
        kind: ActivityKind,
        actor: AgentRef,
        payload: &P,
    ) -> Self {
        let payload = serde_json::to_string(payload).unwrap_or_else(|_| "null".to_string());
        Self {
            id: Uuid::now_v7(),
            session_id,
            kind,
            actor,
            at: Utc::now(),
            payload,
        }
    }

    /// Replace the payload with the JSON-encoded form of `value`.
    pub fn set_payload<P: serde::Serialize>(&mut self, value: &P) -> serde_json::Result<()> {
        self.payload = serde_json::to_string(value)?;
        Ok(())
    }

    /// Parse the JSON-encoded payload into a typed value. Errors
    /// if the stored string isn't valid JSON for `T`.
    pub fn payload_as<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_str(&self.payload)
    }
}
