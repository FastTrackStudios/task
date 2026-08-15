//! Availability schedules and slot queries.
//!
//! [`AvailabilitySchedule`] is a named bundle of weekly
//! [`AvailabilityRule`]s the user maintains ("Working hours",
//! "Open-source weekends"). Slot generation walks the rules
//! intersected with the user's existing [`crate::booking::Booking`]
//! list to produce the free [`TimeSlot`]s the public booking page
//! shows.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::time_block::TimeOfDay;

/// Stable id for an availability schedule (uuid v4 string).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct ScheduleId(pub String);

impl From<String> for ScheduleId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Day of week. Mon..Sun rather than Sun..Sat — same convention as
/// view-calendar, keeps the rest of the app consistent.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum Weekday {
    #[default]
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// One rule: on these days, this start..end window is available.
/// Multiple rules can apply to the same day (e.g. "Mon 9–12 + Mon
/// 13–17" for a lunchtime gap).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct AvailabilityRule {
    pub days: WeekdayList,
    pub start: TimeOfDay,
    pub end: TimeOfDay,
}

/// `Vec<Weekday>` newtype.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct WeekdayList(pub Vec<Weekday>);

impl WeekdayList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<Weekday>> for WeekdayList {
    fn from(v: Vec<Weekday>) -> Self {
        Self(v)
    }
}

impl FromIterator<Weekday> for WeekdayList {
    fn from_iter<I: IntoIterator<Item = Weekday>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for WeekdayList {
    type Target = Vec<Weekday>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for WeekdayList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// `Vec<AvailabilityRule>` newtype.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct AvailabilityRules(pub Vec<AvailabilityRule>);

impl AvailabilityRules {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<AvailabilityRule>> for AvailabilityRules {
    fn from(v: Vec<AvailabilityRule>) -> Self {
        Self(v)
    }
}

impl FromIterator<AvailabilityRule> for AvailabilityRules {
    fn from_iter<I: IntoIterator<Item = AvailabilityRule>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for AvailabilityRules {
    type Target = Vec<AvailabilityRule>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for AvailabilityRules {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Named bundle of rules + an optional IANA timezone identifier.
/// Slot generation interprets the rules in this timezone.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "scheduling_schedules", repo)]
pub struct AvailabilitySchedule {
    /// Vault-relative path of the markdown file backing this
    /// schedule (populated by the scanner). PK.
    #[architect(primary_key, auto_increment = false)]
    pub path: String,
    #[architect(json, filterable)]
    pub id: ScheduleId,
    #[architect(filterable, sortable, fulltext)]
    pub name: String,
    /// e.g. "America/Chicago". `None` = host's local timezone.
    #[architect(filterable)]
    pub timezone: Option<String>,
    #[architect(json)]
    pub rules: AvailabilityRules,
}

/// Inclusive start, exclusive end. Both are ISO-8601 UTC strings
/// (`2026-05-22T14:30:00Z`) — we use strings on the wire so
/// chrono / time stays a consumer-side concern.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct TimeSlot {
    pub start_utc: String,
    pub end_utc: String,
}

/// Slot-listing parameters for the public booking page.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct SlotQuery {
    pub event_type_id: crate::event_type::EventTypeId,
    /// Inclusive UTC start of the search window.
    pub from_utc: String,
    /// Exclusive UTC end.
    pub to_utc: String,
}
