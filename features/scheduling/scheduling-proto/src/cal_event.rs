//! Persisted calendar event — what the user drops onto the schedule.
//!
//! Distinct from [`crate::EventType`] (a cal.com-style bookable
//! template) and [`crate::DayPlan`] blocks (the day's structure): a
//! `CalEvent` is one concrete event on the timeline. The
//! `view-calendar` UI type maps onto this for storage. Times are
//! RFC-3339 strings and `color` is the `ColorTag` name so the proto
//! stays serializer-agnostic and `Facet`-encodable.

use facet::Facet;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "scheduling_cal_events", repo)]
pub struct CalEvent {
    /// Stable id (uuid string). PK — the vault file is `<id>.md`.
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    #[architect(filterable, sortable, fulltext)]
    pub title: String,
    /// Inclusive start, RFC-3339.
    #[architect(filterable, sortable)]
    pub start: String,
    /// Exclusive end, RFC-3339.
    #[architect(filterable, sortable)]
    pub end: String,
    #[architect(filterable)]
    pub all_day: bool,
    /// `ColorTag` name (`"primary"`, `"success"`, …).
    #[architect(filterable)]
    pub color: String,
    #[architect(fulltext)]
    pub description: Option<String>,
    /// RFC-5545 RRULE, if recurring.
    pub recurrence: Option<String>,
}
