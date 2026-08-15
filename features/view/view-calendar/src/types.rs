//! Core data types — flat single-calendar model. Times are
//! `DateTime<Utc>` on the wire and rendered in the user's local
//! timezone at the component boundary (defer real TZ handling).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type EventId = Uuid;

/// View mode controlled by the root `Calendar` toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    Month,
    #[default]
    Week,
    Day,
    /// A week whose vertical axis is *categories* rather than hours —
    /// breakfast/lunch/dinner, morning/afternoon/evening, shift names.
    ///
    /// The other modes place a block by when it happens; this one
    /// places it by what kind of thing it is. Some plans genuinely
    /// have no clock value — "Friday dinner" is not 18:30 — and
    /// inventing one to satisfy a time axis makes the grid lie.
    Blocks,
}

impl ViewMode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Month => "Month",
            Self::Week => "Week",
            Self::Day => "Day",
            Self::Blocks => "Blocks",
        }
    }
}

/// Event color tag — same vocabulary as view-kanban so consumers
/// can share status palettes. Defer per-event hex colors until
/// the design system says we need them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorTag {
    Neutral,
    #[default]
    Primary,
    Success,
    Warning,
    Danger,
    Info,
}

impl ColorTag {
    /// Tailwind color stem (e.g. `"violet"`).
    #[must_use]
    pub fn stem(self) -> &'static str {
        match self {
            Self::Neutral => "slate",
            Self::Primary => "violet",
            Self::Success => "emerald",
            Self::Warning => "amber",
            Self::Danger => "rose",
            Self::Info => "sky",
        }
    }
}

/// Payload of a plan-block grid drag: `(origin_date, target_date,
/// block_id, start_min, end_min)`. The origin/target split lets a Move
/// carry a block to a different day.
pub type BlockEdit = (chrono::NaiveDate, chrono::NaiveDate, String, u16, u16);

/// A day-plan block rendered behind real events as a placement guide
/// — a faded outline (e.g. "Block 1: Work / Event / Free Time"
/// 9:30–12:30) for one specific date. Clicking it emits `on_block_click`
/// so the consumer can edit the day's plan (move/relabel/assign). The
/// user also drops actual [`CalendarEvent`]s onto these outlines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateBlock {
    /// Stable id within the date's plan — what `on_block_click`
    /// reports so the consumer can target the edit.
    pub id: String,
    /// The specific date this block sits on.
    pub date: chrono::NaiveDate,
    pub label: String,
    /// Minutes since local midnight, `[0, 1440]`. `end_min` is
    /// exclusive. A block crossing midnight is split by the producer
    /// into two same-day blocks.
    pub start_min: u16,
    pub end_min: u16,
    pub color: ColorTag,
    /// Optional second line — what's assigned to the block today
    /// (a task title, project, or free label). `None` = unassigned.
    pub assignment: Option<String>,
    /// Soft block — template fallback for a day with no explicit
    /// plan (or the unedited remainder of a sparse one). Rendered
    /// dashed + faded so it reads as "planned shape", not a
    /// confirmed block.
    pub soft: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: EventId,
    pub title: String,
    /// Inclusive event start (UTC).
    pub start: DateTime<Utc>,
    /// Exclusive event end (UTC). For all-day events this is
    /// midnight of the day *after* the last day shown.
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub color: ColorTag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional RFC-5545 RRULE string (e.g. `"FREQ=WEEKLY;BYDAY=MO,WE"`).
    /// The `start`/`end` window is the *first* occurrence; later
    /// occurrences are expanded at view time. For v1, editing or
    /// dragging an instance edits the master (and therefore the
    /// whole series).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<String>,
}

impl CalendarEvent {
    pub fn new(title: impl Into<String>, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            start,
            end,
            all_day: false,
            color: ColorTag::default(),
            description: None,
            recurrence: None,
        }
    }

    /// Duration in whole minutes. Used by the time-grid views to
    /// compute the rendered height.
    #[must_use]
    pub fn duration_minutes(&self) -> i64 {
        (self.end - self.start).num_minutes().max(0)
    }
}
