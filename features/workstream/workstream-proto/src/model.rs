//! `Workstream` — the project-scoped parent-with-swarm construct.
//!
//! Replaces the `epic` tag convention with a real entity. Plane
//! splits this into Module (lead + members + status + dates) and
//! Epic (issue-type with child rollup); Task builds **one**
//! construct:
//!
//! - **Workstream** carries the orchestration surface: a lead
//!   (the orchestrator — human or agent), members (the swarm),
//!   status, and start/target dates.
//! - Tasks attach via `task::WorkflowAttrs::workstream`
//!   (`Option<Uuid>`); progress is a *derived* rollup over those
//!   tasks (done / total / in-progress / blocked / estimate sum),
//!   never stored.
//!
//! Distinct from `milestone::Milestone` (a date-anchored
//! checkpoint that maps 1:1 onto GH/Forgejo milestones) and from
//! `goal::Goal` (a life-horizon ambition). A workstream is a
//! *stream of work* with a crew attached.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use workflows_proto::AgentRef;

/// `Vec<AgentRef>` newtype — JSON column. The workstream's swarm:
/// every actor (human or agent) working under this stream. Same
/// shape as `task::model::AgentRefList`, redeclared here so the
/// wasm-clean proto doesn't pull the task crate.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct AgentRefList(pub Vec<AgentRef>);

impl AgentRefList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<AgentRef>> for AgentRefList {
    fn from(v: Vec<AgentRef>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for AgentRefList {
    type Target = Vec<AgentRef>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for AgentRefList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// `Vec<String>` newtype for the workstream's reference links
/// (PRDs, design docs, dashboards). JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Links(pub Vec<String>);

impl Links {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for Links {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for Links {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "workstreams", repo)]
pub struct Workstream {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub title: String,

    /// Owning project. Required — a workstream always lives
    /// inside one project. The backend derives the on-disk
    /// location (`Projects/<slug>/workstreams/<ws-slug>.md`)
    /// from this when `path` is empty on create.
    #[architect(filterable, sortable)]
    #[serde(rename = "projectId")]
    pub project_id: Uuid,

    /// Lifecycle status — one of the canonical [`Status`] slugs
    /// (`backlog` / `planned` / `in-progress` / `paused` /
    /// `done` / `cancelled`). Stored as a string so vault pages
    /// stay hand-editable; [`Status::from_str`] canonicalizes.
    #[serde(default = "default_status")]
    #[architect(filterable, sortable)]
    pub status: String,

    /// The orchestrator — who runs this stream. Human or agent.
    /// `None` while unassigned (backlog grooming).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(json)]
    pub lead: Option<AgentRef>,

    /// The swarm — every actor working under this stream. The
    /// lead is *not* implicitly a member; list them explicitly
    /// if they also execute.
    #[serde(skip_serializing_if = "AgentRefList::is_empty", default)]
    #[architect(json)]
    pub members: AgentRefList,

    /// When work starts (planned or actual). `None` = backlog.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "startDate")]
    #[architect(filterable, sortable)]
    pub start_date: Option<NaiveDate>,

    /// Target completion date. `None` for open-ended streams.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "targetDate"
    )]
    #[architect(filterable, sortable)]
    pub target_date: Option<NaiveDate>,

    /// Reference links (PRDs, design docs, dashboards).
    #[serde(skip_serializing_if = "Links::is_empty", default)]
    #[architect(json)]
    pub links: Links,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateCreated"
    )]
    pub date_created: Option<DateTime<Utc>>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateModified"
    )]
    pub date_modified: Option<DateTime<Utc>>,

    /// Markdown body — the workstream's PRD / charter.
    #[serde(skip)]
    pub details: String,
}

fn default_status() -> String {
    Status::Backlog.as_str().to_string()
}

/// Canonical workstream lifecycle. Parsing accepts the common
/// aliases; unknown strings are kept raw on the model (same
/// tolerance as `task::Status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Captured but not yet scheduled.
    Backlog,
    /// Scheduled — has dates / a crew, work not started.
    Planned,
    /// Work happening now.
    InProgress,
    /// Deliberately on hold (kept distinct from cancelled so
    /// the rollup history survives a resume).
    Paused,
    /// Shipped.
    Done,
    /// Abandoned.
    Cancelled,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Planned => "planned",
            Self::InProgress => "in-progress",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "backlog" | "triage" => Some(Self::Backlog),
            "planned" | "todo" | "scheduled" => Some(Self::Planned),
            "in-progress" | "in_progress" | "active" | "doing" => Some(Self::InProgress),
            "paused" | "on-hold" | "on_hold" | "blocked" => Some(Self::Paused),
            "done" | "completed" | "complete" | "shipped" => Some(Self::Done),
            "cancelled" | "canceled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// `true` once the stream is over (done or cancelled).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}
