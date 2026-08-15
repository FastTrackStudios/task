//! RRULE expansion.
//!
//! Events carry an optional [`CalendarEvent::recurrence`] string
//! (RFC-5545 RRULE, e.g. `"FREQ=WEEKLY;BYDAY=MO,WE"`). At render
//! time the view component calls [`expand_in_range`] to turn each
//! master into a fan of instances that fall inside the visible
//! window. Each instance is the master cloned with its `start`/`end`
//! shifted to the occurrence's time; the `id` is preserved so any
//! mutation triggered from an instance flows back to the master.
//!
//! The actual RRULE engine is the shared
//! [`scheduling_proto::recurrence`] module — the CLI (`task plan`)
//! expands through the same code, so a rule materializes the same
//! instances on both surfaces.
//!
//! For v1, editing or dragging an instance edits the *whole
//! series*. Per-instance exceptions ("this and future" / "just
//! this") are a follow-up that needs an `exdates` + an
//! "override events" table on the state.

use chrono::{DateTime, Utc};
use scheduling_proto::recurrence::expand_rrule;

use crate::types::CalendarEvent;

/// Expand `event` into all instances that overlap
/// `[range_start, range_end)`. Non-recurring events return
/// `vec![event]` when they overlap and `vec![]` otherwise. Bad
/// RRULE strings degrade to a single-instance render (the master)
/// so a typo doesn't make the event disappear.
#[must_use]
pub fn expand_in_range(
    event: &CalendarEvent,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
) -> Vec<CalendarEvent> {
    let overlaps_master = event.end > range_start && event.start < range_end;
    let master_only = || {
        if overlaps_master {
            vec![event.clone()]
        } else {
            vec![]
        }
    };
    let Some(rule_str) = event.recurrence.as_deref() else {
        return master_only();
    };
    let Some(starts) = expand_rrule(rule_str, event.start, range_start, range_end) else {
        // Parse / validation failure — fall back to the bare master.
        return master_only();
    };

    let duration = event.end - event.start;
    starts
        .into_iter()
        .map(|start| {
            let mut inst = event.clone();
            inst.start = start;
            inst.end = start + duration;
            inst
        })
        .collect()
}

/// Apply [`expand_in_range`] to each event in `events` and
/// flatten the results.
#[must_use]
pub fn expand_all(
    events: &[CalendarEvent],
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
) -> Vec<CalendarEvent> {
    events
        .iter()
        .flat_map(|ev| expand_in_range(ev, range_start, range_end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn no_recurrence_returns_master_when_in_range() {
        let mut ev = CalendarEvent::new("standup", at(2026, 5, 4, 9), at(2026, 5, 4, 10));
        ev.recurrence = None;
        let out = expand_in_range(&ev, at(2026, 5, 1, 0), at(2026, 5, 8, 0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start, ev.start);
    }

    #[test]
    fn weekly_expands_within_window() {
        let mut ev = CalendarEvent::new("standup", at(2026, 5, 4, 9), at(2026, 5, 4, 10));
        ev.recurrence = Some("FREQ=WEEKLY;BYDAY=MO,WE,FR".into());
        let out = expand_in_range(&ev, at(2026, 5, 4, 0), at(2026, 5, 11, 0));
        // Mon May 4, Wed May 6, Fri May 8 = 3 instances.
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].start - out[0].start, Duration::days(2));
        assert!(out.iter().all(|i| i.id == ev.id));
    }

    #[test]
    fn biweekly_expands_every_other_week() {
        let mut ev = CalendarEvent::new("payday", at(2026, 5, 4, 9), at(2026, 5, 4, 10));
        ev.recurrence = Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO".into());
        let out = expand_in_range(&ev, at(2026, 5, 4, 0), at(2026, 6, 1, 0));
        // May 4 and May 18 — not May 11 / 25 (25th is in week 3).
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].start - out[0].start, Duration::days(14));
    }

    #[test]
    fn invalid_rrule_falls_back_to_master() {
        let mut ev = CalendarEvent::new("standup", at(2026, 5, 4, 9), at(2026, 5, 4, 10));
        ev.recurrence = Some("NOT A REAL RULE".into());
        let out = expand_in_range(&ev, at(2026, 5, 1, 0), at(2026, 5, 8, 0));
        assert_eq!(out.len(), 1);
    }
}
