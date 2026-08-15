//! Shared RFC-5545 RRULE expansion — the single recurrence engine
//! for every surface (the `view-calendar` UI and `task plan`'s
//! CLI verbs both delegate here, so a rule expands identically on
//! both).
//!
//! Built on the `rrule` crate, so the full grammar is supported:
//! `FREQ=WEEKLY`, `INTERVAL=2` (biweekly), `FREQ=MONTHLY`, BYDAY
//! lists, COUNT/UNTIL, … Occurrences are computed in the **local
//! timezone** (the rule's DTSTART is the event's start instant
//! converted to local time), so `BYDAY=SU` means the user's
//! Sunday even when the UTC instant falls on a different weekday.
//!
//! Callers treat a `None` from [`expand_rrule`] as "bad rule" and
//! degrade to a single master occurrence — a typo never makes an
//! event disappear.

use chrono::{DateTime, Utc};
use rrule::{RRule, RRuleSet, Tz, Unvalidated};

/// Maximum instances to return per expansion. Guards against
/// runaway `FREQ=SECONDLY`-style rules; generous enough that a
/// year of weekly events expands without hitting the cap.
pub const MAX_INSTANCES: u16 = 1024;

/// Expand `rule` (an RFC-5545 RRULE string, no `RRULE:` prefix)
/// anchored at `dt_start` into the occurrence starts that fall
/// within `[range_start, range_end]`. Returns `None` when the
/// rule doesn't parse or validate — callers fall back to the
/// master occurrence.
#[must_use]
pub fn expand_rrule(
    rule: &str,
    dt_start: DateTime<Utc>,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
) -> Option<Vec<DateTime<Utc>>> {
    let parsed: RRule<Unvalidated> = rule.parse().ok()?;
    // Local-time anchor: BYDAY / BYMONTHDAY resolve against the
    // user's calendar, not UTC's.
    let anchor = dt_start.with_timezone(&Tz::LOCAL);
    let validated = parsed.validate(anchor).ok()?;
    let set = RRuleSet::new(anchor)
        .rrule(validated)
        .after(range_start.with_timezone(&Tz::LOCAL))
        .before(range_end.with_timezone(&Tz::LOCAL));
    Some(
        set.all(MAX_INSTANCES)
            .dates
            .into_iter()
            .map(|d| d.with_timezone(&Utc))
            .collect(),
    )
}

/// Human label for a rule's cadence: `"weekly"`, `"biweekly"`,
/// `"monthly"`, `"daily"`, `"yearly"`, or `"every N <unit>s"`.
/// `None` when FREQ is missing/unrecognized (sub-daily rules are
/// nobody's schedule surface).
#[must_use]
pub fn describe_rrule(rule: &str) -> Option<String> {
    let mut freq: Option<String> = None;
    let mut interval: u32 = 1;
    for part in rule.split(';') {
        let (k, v) = part.split_once('=')?;
        match k.trim().to_ascii_uppercase().as_str() {
            "FREQ" => freq = Some(v.trim().to_ascii_uppercase()),
            "INTERVAL" => interval = v.trim().parse().ok()?,
            _ => {}
        }
    }
    let unit = match freq?.as_str() {
        "DAILY" => "day",
        "WEEKLY" => "week",
        "MONTHLY" => "month",
        "YEARLY" => "year",
        _ => return None,
    };
    Some(match (unit, interval) {
        ("day", 1) => "daily".into(),
        ("week", 1) => "weekly".into(),
        ("week", 2) => "biweekly".into(),
        ("month", 1) => "monthly".into(),
        ("year", 1) => "yearly".into(),
        (u, n) => format!("every {n} {u}s"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        // Anchor via the local timezone so weekday semantics match
        // what the CLI / UI do with user-entered times.
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn weekly_expands_on_bydays() {
        // Mon 2026-05-04 09:00, Mon/Wed/Fri.
        let out = expand_rrule(
            "FREQ=WEEKLY;BYDAY=MO,WE,FR",
            at(2026, 5, 4, 9),
            at(2026, 5, 4, 0),
            at(2026, 5, 11, 0),
        )
        .unwrap();
        assert_eq!(out.len(), 3); // Mon 4, Wed 6, Fri 8
        assert_eq!(out[1] - out[0], chrono::Duration::days(2));
    }

    #[test]
    fn biweekly_skips_the_off_week() {
        // Sun 2026-06-14 09:00, every 2 weeks.
        let rule = "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU";
        let anchor = at(2026, 6, 14, 9);
        let june = expand_rrule(rule, anchor, at(2026, 6, 14, 0), at(2026, 6, 29, 0)).unwrap();
        // 6/14 and 6/28 — NOT 6/21.
        assert_eq!(june.len(), 2);
        assert_eq!(june[1] - june[0], chrono::Duration::days(14));
        let off_week = expand_rrule(rule, anchor, at(2026, 6, 21, 0), at(2026, 6, 22, 0)).unwrap();
        assert!(off_week.is_empty());
    }

    #[test]
    fn monthly_repeats_on_the_anchor_day() {
        let rule = "FREQ=MONTHLY";
        let anchor = at(2026, 6, 14, 9);
        let out = expand_rrule(rule, anchor, at(2026, 6, 14, 0), at(2026, 9, 1, 0)).unwrap();
        assert_eq!(out.len(), 3); // 6/14, 7/14, 8/14
        let none = expand_rrule(rule, anchor, at(2026, 7, 15, 0), at(2026, 7, 16, 0)).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn bad_rules_return_none() {
        let anchor = at(2026, 6, 14, 9);
        assert!(expand_rrule("NOT A RULE", anchor, at(2026, 6, 1, 0), at(2026, 7, 1, 0)).is_none());
    }

    #[test]
    fn describes_common_cadences() {
        assert_eq!(
            describe_rrule("FREQ=WEEKLY;BYDAY=SU").as_deref(),
            Some("weekly")
        );
        assert_eq!(
            describe_rrule("FREQ=WEEKLY;INTERVAL=2;BYDAY=SU").as_deref(),
            Some("biweekly")
        );
        assert_eq!(describe_rrule("FREQ=MONTHLY").as_deref(), Some("monthly"));
        assert_eq!(describe_rrule("FREQ=DAILY").as_deref(), Some("daily"));
        assert_eq!(
            describe_rrule("FREQ=WEEKLY;INTERVAL=3").as_deref(),
            Some("every 3 weeks")
        );
        assert_eq!(describe_rrule("FREQ=SECONDLY"), None);
        assert_eq!(describe_rrule("garbage"), None);
    }
}
