//! Reactive store + mutation vocabulary.
//!
//! `CalendarState` is plain data. The root `Calendar` holds it
//! in a `Signal<CalendarState>`. Mutations from inner components
//! bubble up as [`CalendarMutation`] through `on_event` — the
//! *consumer* decides whether to write through to a CRDT, debounce,
//! etc. (Per AGENTS.md: dumb components, data in, events out.)

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::types::{CalendarEvent, ColorTag, EventId};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarState {
    /// Events keyed by id. `IndexMap` preserves insertion order so
    /// snapshots are deterministic; per-day ordering for the views
    /// is computed at layout time by sorting on `start`.
    pub events: IndexMap<EventId, CalendarEvent>,
}

impl CalendarState {
    /// Events whose `[start, end)` interval intersects `[range_start,
    /// range_end)`. Cheap O(n) scan — fine for v1 single-calendar
    /// loads; swap for an interval-tree if a vault drops 10k events
    /// on us.
    #[must_use]
    pub fn events_in_range(
        &self,
        range_start: DateTime<Utc>,
        range_end: DateTime<Utc>,
    ) -> Vec<CalendarEvent> {
        self.events
            .values()
            .filter(|ev| ev.end > range_start && ev.start < range_end)
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarMutation {
    /// Brand-new event. Carries the full payload because the
    /// consumer chooses the id (lets architect/CRDT mint it).
    Create {
        event: CalendarEvent,
    },
    /// Drag-move OR drag-resize collapse to the same shape — both
    /// just rewrite the `[start, end)` window. Inner components
    /// emit this; the consumer treats it uniformly.
    Reschedule {
        id: EventId,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    Rename {
        id: EventId,
        title: String,
    },
    Recolor {
        id: EventId,
        color: ColorTag,
    },
    SetAllDay {
        id: EventId,
        all_day: bool,
    },
    SetDescription {
        id: EventId,
        description: Option<String>,
    },
    /// Set or clear the RRULE string. Empty or `None` removes
    /// recurrence; non-empty is parsed at expand time so a bad
    /// string degrades to "single-instance" rather than a hard
    /// error.
    SetRecurrence {
        id: EventId,
        recurrence: Option<String>,
    },
    Remove {
        id: EventId,
    },
}

/// In-place reducer matching the mutation vocabulary. Used by the
/// demo route as a drop-in stand-in for a CRDT-backed wrapper.
/// Invalid ids are silently ignored.
pub fn apply(state: &mut CalendarState, mu: &CalendarMutation) {
    match mu {
        CalendarMutation::Create { event } => {
            state.events.insert(event.id, event.clone());
        }
        CalendarMutation::Reschedule { id, start, end } => {
            if let Some(ev) = state.events.get_mut(id) {
                ev.start = *start;
                ev.end = *end;
            }
        }
        CalendarMutation::Rename { id, title } => {
            if let Some(ev) = state.events.get_mut(id) {
                ev.title = title.clone();
            }
        }
        CalendarMutation::Recolor { id, color } => {
            if let Some(ev) = state.events.get_mut(id) {
                ev.color = *color;
            }
        }
        CalendarMutation::SetAllDay { id, all_day } => {
            if let Some(ev) = state.events.get_mut(id) {
                ev.all_day = *all_day;
            }
        }
        CalendarMutation::SetDescription { id, description } => {
            if let Some(ev) = state.events.get_mut(id) {
                ev.description = description.clone();
            }
        }
        CalendarMutation::SetRecurrence { id, recurrence } => {
            if let Some(ev) = state.events.get_mut(id) {
                ev.recurrence = recurrence
                    .as_ref()
                    .filter(|s| !s.trim().is_empty())
                    .cloned();
            }
        }
        CalendarMutation::Remove { id } => {
            state.events.shift_remove(id);
        }
    }
}
