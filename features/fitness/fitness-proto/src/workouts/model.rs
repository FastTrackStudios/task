//! Workout models — `Routine` (template), `WorkoutSession`
//! (one instance, programmed or ad-hoc), `LoggedSet` (the
//! actuals).
//!
//! Modeled on wger's `Routine` → `Day` → `Slot` →
//! `SlotEntry` hierarchy + `WorkoutSession` + `WorkoutLog`
//! tables, flattened so each routine and each session is
//! one markdown page. The hierarchy stays nested in
//! frontmatter — `Routine.days[].slots[]` — so the page
//! reads sensibly in a text editor.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `Vec<String>` newtype — JSON column.
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

/// `Vec<RoutineDay>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct RoutineDays(pub Vec<RoutineDay>);

impl RoutineDays {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<RoutineDay>> for RoutineDays {
    fn from(v: Vec<RoutineDay>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for RoutineDays {
    type Target = Vec<RoutineDay>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// `Vec<LoggedSet>` newtype — JSON column. Logged sets live
/// inline in the session row; if per-set queries get hot,
/// promote `LoggedSet` to its own table.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct LoggedSets(pub Vec<LoggedSet>);

impl LoggedSets {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<LoggedSet>> for LoggedSets {
    fn from(v: Vec<LoggedSet>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for LoggedSets {
    type Target = Vec<LoggedSet>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for LoggedSets {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// ── Routine (programmed template) ─────────────────────────

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "routines", repo)]
pub struct Routine {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,

    /// Days in the rotation. A routine with one entry is a
    /// full-body program; one with three is a typical
    /// push/pull/legs split.
    #[serde(skip_serializing_if = "RoutineDays::is_empty", default)]
    #[architect(json)]
    pub days: RoutineDays,

    /// Free-form tags — `"strength"`, `"hypertrophy"`,
    /// `"in-season"`, `"deload"`.
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

    #[serde(skip)]
    pub details: String,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct RoutineDay {
    /// Display label — `"Push Day"`, `"Day A"`,
    /// `"Heavy Squats"`.
    pub name: String,

    /// Ordered slots. A slot groups exercises that share
    /// programming (a straight set vs a superset). Most
    /// days are single-exercise slots.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub slots: Vec<RoutineSlot>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

/// One slot — programmed sets of a single exercise (or
/// supersetted exercises). Mirrors wger's `Slot` +
/// `SlotEntry` collapsed into one row when the slot only
/// holds one exercise (the common case).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct RoutineSlot {
    /// Exercise from the `exercises` catalog. Cached
    /// `exercise_name` for display when the catalog isn't
    /// loaded.
    #[serde(rename = "exerciseId")]
    pub exercise_id: Uuid,

    #[serde(rename = "exerciseName")]
    pub exercise_name: String,

    /// Number of working sets. `None` for AMRAP / "to
    /// failure" sets where the count isn't programmed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sets: Option<u32>,

    /// Programmed rep target. Free-form string so ranges
    /// (`"8-12"`), AMRAP (`"AMRAP"`), and "to failure"
    /// round-trip without inventing a sum type.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reps: Option<String>,

    /// Programmed working weight in kilograms. Convention
    /// only; the log records actuals.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "weightKg")]
    pub weight_kg: Option<f64>,

    /// Reps in reserve target — `0` = "to failure",
    /// `3` = "stop 3 short".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rir: Option<u32>,

    /// Rest between sets in seconds.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "restSeconds"
    )]
    pub rest_seconds: Option<u32>,

    /// Optional cue — `"pause 2s at chest"`,
    /// `"tempo 3-1-1"`, `"superset with curls"`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

// ── WorkoutSession (one performance) ──────────────────────

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "workout_sessions", repo)]
pub struct WorkoutSession {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Display label. Falls back to `"<routine name> —
    /// <day>"` or just the date when neither is known.
    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Calendar date the workout was performed on. Stored
    /// as a date rather than full timestamp so weekly /
    /// monthly views read cleanly; per-set `started_at` /
    /// `ended_at` live on the session if needed.
    #[architect(filterable, sortable)]
    pub date: NaiveDate,

    /// Optional routine this session was performed against.
    /// `None` for ad-hoc workouts — just a list of logged
    /// sets, no template.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "routineId")]
    #[architect(filterable)]
    pub routine_id: Option<Uuid>,

    /// Which day from the routine — `"Push Day"`. Free-form
    /// to mirror [`RoutineDay::name`].
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "dayName")]
    pub day_name: Option<String>,

    /// Actuals: every set performed in this session. Flat
    /// list — `LoggedSet::order` keeps sequence; multiple
    /// sets of the same exercise are sequential rows.
    #[serde(
        skip_serializing_if = "LoggedSets::is_empty",
        default,
        rename = "loggedSets"
    )]
    #[architect(json)]
    pub logged_sets: LoggedSets,

    /// Free-form status. Canonical set in [`SessionStatus`]
    /// (`planned` / `in-progress` / `completed` /
    /// `skipped`).
    #[serde(default = "default_status")]
    #[architect(filterable)]
    pub status: String,

    /// Total session duration in minutes. Optional —
    /// many lifters don't time it.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "durationMinutes"
    )]
    pub duration_minutes: Option<u32>,

    /// Tags — `"home-gym"`, `"travel"`, `"PR-day"`,
    /// `"deload"`.
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

    /// Markdown body — pre-workout state, post-workout
    /// notes, how each lift felt.
    #[serde(skip)]
    pub details: String,
}

fn default_status() -> String {
    SessionStatus::Completed.as_str().to_string()
}

/// One actual set performed. The level wger calls
/// `WorkoutLog`. Held inline inside
/// [`WorkoutSession::logged_sets`] as JSON; promotes to its
/// own entity once per-set queries get hot.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct LoggedSet {
    /// Stable id so a UI can edit individual rows without
    /// re-keying.
    pub id: Uuid,

    #[serde(rename = "exerciseId")]
    pub exercise_id: Uuid,

    /// Cached display name — round-trips when the exercise
    /// catalog isn't loaded.
    #[serde(rename = "exerciseName")]
    pub exercise_name: String,

    /// Sequence within the session — 0-based. Multiple
    /// sets of the same exercise increment normally
    /// (`order: 0, 1, 2`); supersetted sets interleave.
    pub order: u32,

    pub reps: u32,

    /// Weight in kilograms. Use `0.0` for bodyweight /
    /// assisted sets; carry the assist via `note` or
    /// negative weight (wger uses negative for assistance).
    #[serde(rename = "weightKg")]
    pub weight_kg: f64,

    /// Reps in reserve at the end of the set. `0` =
    /// "couldn't have done one more".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rir: Option<u32>,

    /// Rate of perceived exertion (1.0–10.0). Optional —
    /// many lifters track RIR or RPE but not both.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rpe: Option<f64>,

    /// `false` for sets that were attempted-but-failed
    /// (form breakdown, missed depth, etc.) or skipped
    /// entirely. UI sums treat incomplete sets as 0
    /// volume.
    #[serde(default = "default_completed")]
    pub completed: bool,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

fn default_completed() -> bool {
    true
}

/// Canonical session statuses. Free-form on the wire;
/// canonical set lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionStatus {
    Planned,
    InProgress,
    Completed,
    Skipped,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::InProgress => "in-progress",
            Self::Completed => "completed",
            Self::Skipped => "skipped",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "planned" | "scheduled" => Some(Self::Planned),
            "in-progress" | "in_progress" | "active" => Some(Self::InProgress),
            "completed" | "done" => Some(Self::Completed),
            "skipped" | "cancelled" => Some(Self::Skipped),
            _ => None,
        }
    }
}

impl LoggedSet {
    /// Volume in kg-reps (`weight × reps`). Standard
    /// hypertrophy proxy. Returns `0.0` for incomplete sets.
    pub fn volume(&self) -> f64 {
        if self.completed {
            self.weight_kg * self.reps as f64
        } else {
            0.0
        }
    }
}

impl WorkoutSession {
    /// Total volume across every completed set.
    pub fn total_volume(&self) -> f64 {
        self.logged_sets.iter().map(|s| s.volume()).sum()
    }

    /// Total reps across every completed set.
    pub fn total_reps(&self) -> u32 {
        self.logged_sets
            .iter()
            .filter(|s| s.completed)
            .map(|s| s.reps)
            .sum()
    }
}
