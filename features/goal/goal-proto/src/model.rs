//! `Goal` — typed view of one goal page.
//!
//! Distinct from `project::ProjectInfo`: goals carry a
//! `target_date` and a `kind` that places them on the cyclic
//! life-calendar (`lifetime` ↔ no horizon, `yearly` ↔ one
//! calendar year, `quarterly` ↔ one 13-week quarter, `cycle`
//! ↔ one 28-day cycle, `weekly` ↔ one 7-day week). Larger
//! goals decompose into smaller ones via `parent_id`.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `Vec<String>` newtype — JSON column under SeaORM.
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

impl std::ops::Deref for Tags {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "goals", repo)]
pub struct Goal {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub title: String,

    /// Free-form. Canonical set in [`Kind`].
    /// `lifetime` / `yearly` / `quarterly` / `cycle` /
    /// `weekly` / `other`.
    #[serde(default = "default_kind")]
    #[architect(filterable)]
    pub kind: String,

    /// `aspiration` / `active` / `paused` / `achieved` /
    /// `abandoned`. Free-form so custom states round-trip.
    #[serde(default = "default_status")]
    #[architect(filterable, sortable)]
    pub status: String,

    /// Parent goal, if this one decomposes from a larger
    /// ambition. `None` for top-level / lifetime goals.
    /// Mirrors `Location.parent_id` / `ProjectInfo.parent_id`.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "parentId")]
    #[architect(filterable)]
    pub parent_id: Option<Uuid>,

    /// When this goal is due / aimed for. `None` for
    /// lifetime goals that have no fixed date.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "targetDate"
    )]
    #[architect(filterable, sortable)]
    pub target_date: Option<NaiveDate>,

    /// Optional cycle id this goal is scoped to. Populated
    /// when the cyclic-calendar entities land (Phase 2 in
    /// cyclic life-calendar); meanwhile, callers
    /// can leave it `None` and use `target_date` + `kind`.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "cycleId")]
    #[architect(filterable)]
    pub cycle_id: Option<Uuid>,

    /// Free-form tags. `goal` is conventionally one of them
    /// but not required — the parser uses `type: goal` as
    /// the primary discriminator.
    #[serde(skip_serializing_if = "Tags::is_empty", default)]
    #[architect(json)]
    pub tags: Tags,

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

    /// Markdown body after the frontmatter close fence.
    /// Free-form notes — vision, why-it-matters, success
    /// criteria, decomposition rationale.
    #[serde(skip)]
    pub details: String,
}

fn default_kind() -> String {
    Kind::Lifetime.as_str().to_string()
}

fn default_status() -> String {
    Status::Aspiration.as_str().to_string()
}

/// Canonical `kind` values. Parsing accepts any string —
/// unknown kinds round-trip as the raw string on `Goal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// No fixed horizon — "be a good husband", "buy a house
    /// someday", "stay healthy". Aspirational anchors that
    /// shape decisions but don't have a deadline.
    Lifetime,
    /// Scoped to one calendar year.
    Yearly,
    /// Scoped to one 13-week quarter (see
    /// cyclic life-calendar).
    Quarterly,
    /// Scoped to one 28-day cycle.
    Cycle,
    /// Scoped to one 7-day week.
    Weekly,
    /// Catch-all.
    Other,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lifetime => "lifetime",
            Self::Yearly => "yearly",
            Self::Quarterly => "quarterly",
            Self::Cycle => "cycle",
            Self::Weekly => "weekly",
            Self::Other => "other",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lifetime" | "life" => Some(Self::Lifetime),
            "yearly" | "year" | "annual" => Some(Self::Yearly),
            "quarterly" | "quarter" => Some(Self::Quarterly),
            "cycle" | "cyclic" => Some(Self::Cycle),
            "weekly" | "week" => Some(Self::Weekly),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Canonical `status` values. Parsing accepts any string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Identified but not yet being actively pursued.
    Aspiration,
    /// In flight.
    Active,
    /// Temporarily on hold (deliberate, not abandoned).
    Paused,
    /// Done.
    Achieved,
    /// Walked away from — explicit decision, not silent
    /// neglect. Keep the row for the lessons.
    Abandoned,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aspiration => "aspiration",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Achieved => "achieved",
            Self::Abandoned => "abandoned",
        }
    }
}
