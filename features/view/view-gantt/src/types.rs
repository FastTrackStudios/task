//! Core data types — port of svar `store/src/types.ts`.
//!
//! Field names track the svar shape (`text`, `progress`, `parent`,
//! `open`, `data`) so anyone familiar with the JS API can read this
//! crate. Internal layout state (`$x`, `$y`, `$w`) lives on
//! [`LaidOutTask`] in `store.rs`, not on the input shape.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type TaskId = Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskType {
    #[default]
    Task,
    Summary,
    Milestone,
}

/// Length unit for time scales (granularity of a single cell).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LengthUnit {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl LengthUnit {
    /// Smaller = earlier in this list. Mirrors svar's `units` array.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Minute => 0,
            Self::Hour => 1,
            Self::Day => 2,
            Self::Week => 3,
            Self::Month => 4,
            Self::Quarter => 5,
            Self::Year => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DurationUnit {
    Day,
    Hour,
}

/// One row of the time-scale header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScaleConfig {
    pub unit: LengthUnit,
    pub step: u32,
    /// Strftime-style format string. Applied to the cell-start date.
    pub format: String,
}

impl ScaleConfig {
    pub fn new(unit: LengthUnit, step: u32, format: impl Into<String>) -> Self {
        Self {
            unit,
            step,
            format: format.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkType {
    /// End → Start (finish-to-start). The svar default.
    E2s,
    /// Start → Start.
    S2s,
    /// End → End.
    E2e,
    /// Start → End.
    S2e,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GanttLink {
    pub id: TaskId,
    pub source: TaskId,
    pub target: TaskId,
    #[serde(rename = "type")]
    pub link_type: LinkType,
    #[serde(default)]
    pub lag: i32,
}

/// Input task shape — what the consumer passes in via props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GanttTask {
    pub id: TaskId,
    #[serde(default)]
    pub parent: Option<TaskId>,
    pub text: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub progress: f32,
    #[serde(default, rename = "type")]
    pub task_type: TaskType,
    /// Whether a summary task's children are expanded. `true` = expanded.
    #[serde(default = "default_open")]
    pub open: bool,
    /// If true, this task's bar rolls up onto its parent summary.
    #[serde(default)]
    pub rollup: bool,
    /// Optional details / description.
    #[serde(default)]
    pub details: Option<String>,
    /// Optional bar tint. Any CSS color (`#ff0`, `hsl(...)`, or a
    /// `var(--…)` reference to a theme token). Renders as
    /// background; label color is read from `--gantt-bar-fg` if you
    /// override that token in CSS — defaults to `white` because the
    /// hex demo palette is dark enough for it. For theme-driven
    /// bars, prefer passing a token like `var(--color-primary)` and
    /// let theme presets retint the chart.
    #[serde(default)]
    pub color: Option<String>,
}

fn default_open() -> bool {
    true
}

/// One zoom level: cell-width range + scale rows.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomLevel {
    pub min_cell_width: f32,
    pub max_cell_width: f32,
    pub scales: Vec<ScaleConfig>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZoomConfig {
    pub level: usize,
    pub levels: Vec<ZoomLevel>,
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            level: 3,
            levels: default_zoom_levels(),
        }
    }
}

/// Default zoom levels — year/quarter/month/week/day/hour granularity.
#[must_use]
pub fn default_zoom_levels() -> Vec<ZoomLevel> {
    use LengthUnit::{Day, Hour, Month, Quarter, Week, Year};
    vec![
        ZoomLevel {
            min_cell_width: 200.0,
            max_cell_width: 400.0,
            scales: vec![
                ScaleConfig::new(Year, 1, "%Y"),
                ScaleConfig::new(Quarter, 1, "Q%q %Y"),
            ],
        },
        ZoomLevel {
            min_cell_width: 100.0,
            max_cell_width: 250.0,
            scales: vec![
                ScaleConfig::new(Year, 1, "%Y"),
                ScaleConfig::new(Month, 1, "%b"),
            ],
        },
        ZoomLevel {
            min_cell_width: 80.0,
            max_cell_width: 200.0,
            scales: vec![
                ScaleConfig::new(Month, 1, "%b %Y"),
                ScaleConfig::new(Week, 1, "W%V"),
            ],
        },
        ZoomLevel {
            min_cell_width: 30.0,
            max_cell_width: 120.0,
            scales: vec![
                ScaleConfig::new(Month, 1, "%B %Y"),
                ScaleConfig::new(Day, 1, "%-d"),
            ],
        },
        ZoomLevel {
            min_cell_width: 20.0,
            max_cell_width: 80.0,
            scales: vec![
                ScaleConfig::new(Week, 1, "%b %-d"),
                ScaleConfig::new(Day, 1, "%a"),
            ],
        },
        ZoomLevel {
            min_cell_width: 30.0,
            max_cell_width: 100.0,
            scales: vec![
                ScaleConfig::new(Day, 1, "%b %-d"),
                ScaleConfig::new(Hour, 1, "%H"),
            ],
        },
    ]
}

/// Built-in column kinds the sidebar grid knows how to render.
/// Custom-rendered columns aren't supported here yet — those need a
/// trait-object-with-PartialEq dance we haven't pulled in. Pick from
/// this set or call `Name` and use the tooltip / editor for extras.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnKind {
    Name,
    Start,
    End,
    Progress,
    Duration,
    Type,
}

/// One sidebar column: kind + header label + width.
#[derive(Clone, Debug, PartialEq)]
pub struct GanttColumn {
    pub kind: ColumnKind,
    pub label: String,
    pub width: f32,
}

impl GanttColumn {
    #[must_use]
    pub fn name(width: f32) -> Self {
        Self {
            kind: ColumnKind::Name,
            label: "Task".into(),
            width,
        }
    }
    #[must_use]
    pub fn start(width: f32) -> Self {
        Self {
            kind: ColumnKind::Start,
            label: "Start".into(),
            width,
        }
    }
    #[must_use]
    pub fn end(width: f32) -> Self {
        Self {
            kind: ColumnKind::End,
            label: "End".into(),
            width,
        }
    }
    #[must_use]
    pub fn progress(width: f32) -> Self {
        Self {
            kind: ColumnKind::Progress,
            label: "Progress".into(),
            width,
        }
    }
    #[must_use]
    pub fn duration(width: f32) -> Self {
        Self {
            kind: ColumnKind::Duration,
            label: "Days".into(),
            width,
        }
    }
    #[must_use]
    pub fn type_col(width: f32) -> Self {
        Self {
            kind: ColumnKind::Type,
            label: "Type".into(),
            width,
        }
    }
}

#[must_use]
pub fn default_columns() -> Vec<GanttColumn> {
    vec![GanttColumn::name(280.0)]
}

/// A vertical marker line drawn on the chart (e.g. milestones, deadlines).
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: TaskId,
    pub start: DateTime<Utc>,
    pub text: String,
    /// Optional css class for the marker rule.
    pub css: Option<String>,
}
