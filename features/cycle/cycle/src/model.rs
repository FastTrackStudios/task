//! `Cycle` + `Quarter` — first-class entities for the
//! cyclic life-calendar.

use chrono::NaiveDate;
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One 4-week (28-day) cycle.
///
/// Cycles are identified deterministically via UUIDv5 over
/// `"cycle:<year>:Q<quarter>:C<ordinal>:<start_date>"`, so two
/// machines that compute the same calendar agree on cycle ids
/// without coordination.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "cycles", repo)]
pub struct Cycle {
    #[architect(primary_key, auto_increment = false)]
    pub id: Uuid,

    /// Source year — the calendar year the cycle is anchored
    /// to. A cycle's start_date may fall in the previous
    /// calendar year (e.g. 2026 Q1.C1 starts Dec 29, 2025);
    /// `year` still reads as `2026` because that's the
    /// cyclic year it belongs to.
    #[architect(filterable, sortable)]
    pub year: i32,

    /// Which quarter this cycle sits in. 1..=4.
    #[architect(filterable, sortable)]
    pub quarter: u8,

    /// Which cycle of the quarter. 1..=3.
    #[architect(filterable, sortable)]
    pub ordinal: u8,

    /// Inclusive start (always falls on `week_start_day`).
    #[architect(filterable, sortable)]
    pub start_date: NaiveDate,

    /// Inclusive end (always 27 days after `start_date`).
    #[architect(filterable, sortable)]
    pub end_date: NaiveDate,

    /// Day of week each cycle begins on, stored as the
    /// lowercase short form (`"mon"` / `"tue"` / …).
    /// Configurable per user; Cody uses `"mon"`.
    pub week_start_day: String,
}

/// One 13-week quarter — three [`Cycle`]s plus a reset week,
/// plus an optional bonus "week zero" appended on cyclic-leap
/// years for Q4.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "quarters", repo)]
pub struct Quarter {
    #[architect(primary_key, auto_increment = false)]
    pub id: Uuid,

    #[architect(filterable, sortable)]
    pub year: i32,

    /// 1..=4.
    #[architect(filterable, sortable)]
    pub ordinal: u8,

    /// Inclusive start (= cycle 1 start).
    #[architect(filterable, sortable)]
    pub start_date: NaiveDate,

    /// Inclusive end (= reset_week_end, or bonus_week_end on
    /// the year's final quarter when in a cyclic-leap year).
    #[architect(filterable, sortable)]
    pub end_date: NaiveDate,

    /// Three cycles in calendar order. Always exactly three.
    /// Stored inline rather than via foreign-key so a single
    /// `Quarter` round-trip carries the full structure.
    #[architect(json)]
    pub cycles: Cycles,

    /// 13th week of the quarter — the reset week.
    #[architect(filterable, sortable)]
    pub reset_week_start: NaiveDate,
    #[architect(filterable, sortable)]
    pub reset_week_end: NaiveDate,

    /// Bonus "week zero" appended to Q4 on cyclic-leap years.
    /// Conceptually week zero of the *following* year (see
    /// cyclic life-calendar); attached here because
    /// it lives in calendar time at the end of this quarter.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bonus_week_start: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bonus_week_end: Option<NaiveDate>,
}

/// `Vec<Cycle>` newtype — JSON column under SeaORM.
/// Distinct from architect's auto-emitted `CycleList`
/// pagination response (different shape: that one has
/// `items` / `total` / `page`).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Cycles(pub Vec<Cycle>);

impl Cycles {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<Vec<Cycle>> for Cycles {
    fn from(v: Vec<Cycle>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for Cycles {
    type Target = Vec<Cycle>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// String ↔ `chrono::Weekday` helpers. Callers move between
/// the wire form (`"mon"`) and `Weekday::Mon` using these.
pub fn weekday_to_short(wd: chrono::Weekday) -> &'static str {
    match wd {
        chrono::Weekday::Mon => "mon",
        chrono::Weekday::Tue => "tue",
        chrono::Weekday::Wed => "wed",
        chrono::Weekday::Thu => "thu",
        chrono::Weekday::Fri => "fri",
        chrono::Weekday::Sat => "sat",
        chrono::Weekday::Sun => "sun",
    }
}

#[must_use]
pub fn weekday_from_short(s: &str) -> Option<chrono::Weekday> {
    match s.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Some(chrono::Weekday::Mon),
        "tue" | "tuesday" => Some(chrono::Weekday::Tue),
        "wed" | "wednesday" => Some(chrono::Weekday::Wed),
        "thu" | "thursday" => Some(chrono::Weekday::Thu),
        "fri" | "friday" => Some(chrono::Weekday::Fri),
        "sat" | "saturday" => Some(chrono::Weekday::Sat),
        "sun" | "sunday" => Some(chrono::Weekday::Sun),
        _ => None,
    }
}
