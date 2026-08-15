//! `Milestone` — project-scoped GitHub-Projects-style milestone.
//!
//! Distinct from `goal::Goal`:
//! - **Goal** is a life-horizon ambition; lifetime / yearly /
//!   cycle / etc.  Belongs to the person.
//! - **Milestone** is a project-execution checkpoint. Belongs
//!   to a project; tasks roll up into it. Maps 1:1 with the
//!   Forgejo / GitHub milestone API (title, description,
//!   due_on, state) for the eventual federation sync.
//!
//! A milestone *may* additionally point at a `goal_id` so the
//! rollup chain `task → milestone → project + life-goal` is
//! navigable. Not required.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Tags(pub Vec<String>);

impl Tags {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for Tags {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for Tags {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "milestones", repo)]
pub struct Milestone {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub title: String,

    /// Owning project. Required — a milestone always lives
    /// inside one project. The backend derives the on-disk
    /// location (`Projects/<slug>/milestones/<ms-slug>.md`)
    /// from this when `path` is empty on create.
    #[architect(filterable, sortable)]
    #[serde(rename = "projectId")]
    pub project_id: Uuid,

    /// Optional rollup pointer at a life-goal. Powers the
    /// `task → milestone → goal` chain. None when the
    /// milestone is purely project-execution and doesn't
    /// ladder up to a long-term ambition.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "goalId")]
    #[architect(filterable)]
    pub goal_id: Option<Uuid>,

    /// `open` / `closed`. Mirrors the GitHub / Forgejo
    /// milestone `state` field exactly so the future sync is
    /// a no-op map.
    #[serde(default = "default_status")]
    #[architect(filterable, sortable)]
    pub status: String,

    /// Due date for the milestone (the GH/Forgejo `due_on`).
    /// `None` when no target — common for "themes" /
    /// long-running buckets.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "dueDate")]
    #[architect(filterable, sortable)]
    pub due_date: Option<NaiveDate>,

    /// Free-form tags. `milestone` is the conventional
    /// discriminator but `type: milestone` is what the
    /// parser keys on.
    #[serde(skip_serializing_if = "Tags::is_empty", default)]
    #[architect(json)]
    pub tags: Tags,

    /// Eventual federation pointer at an external system —
    /// e.g. `"forgejo:starcommand.live/FastTrackStudios/task#7"`.
    /// `None` for purely local milestones. Sync logic
    /// reconciles by this field when set.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "forgeRef")]
    #[architect(filterable)]
    pub forge_ref: Option<String>,

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

    /// Markdown body. Maps to GH/Forgejo `description`.
    #[serde(skip)]
    pub details: String,
}

fn default_status() -> String {
    Status::Open.as_str().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// In flight. Maps to GH/Forgejo `open`.
    Open,
    /// Done / shipped. Maps to `closed`.
    Closed,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" | "active" | "in_progress" => Some(Self::Open),
            "closed" | "done" | "shipped" | "complete" | "completed" => Some(Self::Closed),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Milestone {
    type Key = uuid::Uuid;
    fn key(&self) -> uuid::Uuid {
        self.id
    }
}
