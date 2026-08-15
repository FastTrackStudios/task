//! Slot generation. Pure function — caller passes in the event
//! type, the schedule, and existing bookings; we return the open
//! slot list inside the query window.
//!
//! Algorithm:
//! 1. Walk the query window day-by-day.
//! 2. For each day, look at every rule whose `days` includes the
//!    day's weekday. For each rule, walk start..end in `step =
//!    duration + buffer` increments emitting candidate slots.
//! 3. Drop candidates outside `[from, to)`.
//! 4. Drop candidates that overlap a non-cancelled booking.
//! 5. Sort by start, dedup, return.
//!
//! v1 limitations:
//! - All times treated as UTC. The schedule's `timezone` field is
//!   carried through but not yet honored — that comes with the
//!   real `DateTime` layer.
//! - `EventType.min_notice_min` / `max_future_days` aren't on the
//!   proto yet (parity-doc 🔵). When they land, clamp the query
//!   window here.

use chrono::{DateTime, Datelike, Days, Duration, NaiveDate, NaiveTime, TimeZone, Utc};

use scheduling_proto::{
    AvailabilityRule, AvailabilitySchedule, Booking, BookingStatus, EventType, SchedulingError,
    TimeSlot, Weekday,
};

/// `event_type.duration_min + event_type.buffer_min`, in minutes.
#[must_use]
pub fn step_minutes(et: &EventType) -> i64 {
    i64::from(et.duration_min) + i64::from(et.buffer_min)
}

/// Open slots in `[from_utc, to_utc)` for `event_type` according
/// to `schedule`, minus anything that overlaps a live booking
/// (status ∈ {Pending, Confirmed}).
pub fn list_open_slots(
    event_type: &EventType,
    schedule: &AvailabilitySchedule,
    bookings: &[Booking],
    from_utc: DateTime<Utc>,
    to_utc: DateTime<Utc>,
) -> Result<Vec<TimeSlot>, SchedulingError> {
    if event_type.duration_min == 0 {
        return Err(SchedulingError::Invalid {
            field: "event_type.duration_min".into(),
            reason: "must be > 0".into(),
        });
    }
    if to_utc <= from_utc {
        return Ok(Vec::new());
    }

    let duration = Duration::minutes(i64::from(event_type.duration_min));
    let step = Duration::minutes(step_minutes(event_type));
    let live_intervals: Vec<(DateTime<Utc>, DateTime<Utc>)> = bookings
        .iter()
        .filter(|b| matches!(b.status, BookingStatus::Pending | BookingStatus::Confirmed))
        .filter_map(|b| {
            let s = parse_utc(&b.start_utc).ok()?;
            let e = parse_utc(&b.end_utc).ok()?;
            Some((s, e))
        })
        .collect();

    let mut out: Vec<TimeSlot> = Vec::new();
    let mut day = from_utc.date_naive();
    let last_day = to_utc.date_naive();
    while day <= last_day {
        for rule in schedule.rules.iter().filter(|r| rule_covers(r, day)) {
            let mut cursor = day_with_time(day, rule.start.hours(), rule.start.minutes());
            let rule_end = day_with_time(day, rule.end.hours(), rule.end.minutes());
            while cursor + duration <= rule_end {
                let slot_start = cursor;
                let slot_end = cursor + duration;
                if slot_start < to_utc
                    && slot_end > from_utc
                    && !overlaps_any(slot_start, slot_end, &live_intervals)
                {
                    out.push(TimeSlot {
                        start_utc: slot_start.to_rfc3339(),
                        end_utc: slot_end.to_rfc3339(),
                    });
                }
                cursor += step;
            }
        }
        day = match day.checked_add_days(Days::new(1)) {
            Some(d) => d,
            None => break,
        };
    }

    // Stable order: by start time. A naive sort suffices since
    // each rule's emissions are already sorted within a day.
    out.sort_by(|a, b| a.start_utc.cmp(&b.start_utc));
    out.dedup_by(|a, b| a.start_utc == b.start_utc);
    Ok(out)
}

fn rule_covers(rule: &AvailabilityRule, day: NaiveDate) -> bool {
    let target = weekday_of(day);
    rule.days
        .iter()
        .any(|d| std::mem::discriminant(d) == std::mem::discriminant(&target))
}

fn weekday_of(d: NaiveDate) -> Weekday {
    match d.weekday() {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    }
}

fn day_with_time(d: NaiveDate, h: u8, m: u8) -> DateTime<Utc> {
    let t = NaiveTime::from_hms_opt(u32::from(h), u32::from(m), 0).unwrap_or(NaiveTime::MIN);
    Utc.from_utc_datetime(&d.and_time(t))
}

fn overlaps_any(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    intervals: &[(DateTime<Utc>, DateTime<Utc>)],
) -> bool {
    intervals.iter().any(|(s, e)| start < *e && *s < end)
}

pub(crate) fn parse_utc(s: &str) -> Result<DateTime<Utc>, SchedulingError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| SchedulingError::Invalid {
            field: "utc_timestamp".into(),
            reason: format!("not RFC3339: {s}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use scheduling_proto::{
        AvailabilityRule, AvailabilitySchedule, EventType, EventTypeId, EventTypeLocation,
        ScheduleId, TimeOfDay,
    };

    fn working_hours() -> AvailabilitySchedule {
        AvailabilitySchedule {
            id: ScheduleId("wh".into()),
            path: "schedules/wh.md".into(),
            name: "WH".into(),
            timezone: None,
            rules: vec![AvailabilityRule {
                days: vec![
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri,
                ]
                .into(),
                start: TimeOfDay::new(9, 0),
                end: TimeOfDay::new(11, 0),
            }]
            .into(),
        }
    }

    fn consult_30() -> EventType {
        EventType {
            id: EventTypeId("c30".into()),
            path: "event-types/c30.md".into(),
            title: "Consult".into(),
            slug: "c30".into(),
            description: None,
            duration_min: 30,
            buffer_min: 0,
            location: EventTypeLocation::Tbd,
            schedule_id: Some(ScheduleId("wh".into())),
            published: true,
        }
    }

    fn ymd_hms(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    #[test]
    fn one_weekday_yields_four_30_min_slots() {
        // 2026-06-01 is a Monday. Window covers just that morning.
        let slots = list_open_slots(
            &consult_30(),
            &working_hours(),
            &[],
            ymd_hms(2026, 6, 1, 0, 0),
            ymd_hms(2026, 6, 2, 0, 0),
        )
        .unwrap();
        // 09:00, 09:30, 10:00, 10:30 → 4 slots ending at 11:00.
        assert_eq!(slots.len(), 4);
        assert!(slots[0].start_utc.starts_with("2026-06-01T09:00"));
        assert!(slots[3].start_utc.starts_with("2026-06-01T10:30"));
    }

    #[test]
    fn weekend_yields_no_slots() {
        // 2026-06-06 is a Saturday.
        let slots = list_open_slots(
            &consult_30(),
            &working_hours(),
            &[],
            ymd_hms(2026, 6, 6, 0, 0),
            ymd_hms(2026, 6, 7, 0, 0),
        )
        .unwrap();
        assert!(slots.is_empty());
    }

    #[test]
    fn booking_blocks_overlapping_slots() {
        use scheduling_proto::{Booking, BookingId};
        // Mon 09:15–09:45 booking should remove the 09:00 and
        // 09:30 candidates (both overlap), but leave 10:00 + 10:30.
        let b = Booking {
            id: BookingId("b1".into()),
            path: "bookings/b1.md".into(),
            event_type_id: EventTypeId("c30".into()),
            start_utc: "2026-06-01T09:15:00+00:00".into(),
            end_utc: "2026-06-01T09:45:00+00:00".into(),
            attendee_name: "Bob".into(),
            attendee_email: "bob@x".into(),
            note: None,
            status: BookingStatus::Confirmed,
            created_utc: String::new(),
        };
        let slots = list_open_slots(
            &consult_30(),
            &working_hours(),
            std::slice::from_ref(&b),
            ymd_hms(2026, 6, 1, 0, 0),
            ymd_hms(2026, 6, 2, 0, 0),
        )
        .unwrap();
        let starts: Vec<&str> = slots.iter().map(|s| s.start_utc.as_str()).collect();
        assert!(!starts.iter().any(|s| s.starts_with("2026-06-01T09:00")));
        assert!(!starts.iter().any(|s| s.starts_with("2026-06-01T09:30")));
        assert!(starts.iter().any(|s| s.starts_with("2026-06-01T10:00")));
        assert!(starts.iter().any(|s| s.starts_with("2026-06-01T10:30")));
    }

    #[test]
    fn cancelled_booking_doesnt_block() {
        use scheduling_proto::{Booking, BookingId};
        let b = Booking {
            id: BookingId("b1".into()),
            path: "bookings/b1.md".into(),
            event_type_id: EventTypeId("c30".into()),
            start_utc: "2026-06-01T09:00:00+00:00".into(),
            end_utc: "2026-06-01T09:30:00+00:00".into(),
            attendee_name: "Bob".into(),
            attendee_email: "bob@x".into(),
            note: None,
            status: BookingStatus::Cancelled,
            created_utc: String::new(),
        };
        let slots = list_open_slots(
            &consult_30(),
            &working_hours(),
            std::slice::from_ref(&b),
            ymd_hms(2026, 6, 1, 0, 0),
            ymd_hms(2026, 6, 2, 0, 0),
        )
        .unwrap();
        assert_eq!(slots.len(), 4);
    }

    #[test]
    fn buffer_min_widens_step() {
        let mut et = consult_30();
        et.buffer_min = 15;
        // step = 45 min; 9:00, 9:45, 10:30 → 3 slots.
        let slots = list_open_slots(
            &et,
            &working_hours(),
            &[],
            ymd_hms(2026, 6, 1, 0, 0),
            ymd_hms(2026, 6, 2, 0, 0),
        )
        .unwrap();
        assert_eq!(slots.len(), 3);
    }
}
