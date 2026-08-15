//! Pure-function calendar generator.
//!
//! Given a year + the user's preferred week-start day + a
//! rule for picking week 1, computes the full cyclic
//! calendar: 4 [`Quarter`]s × 3 [`Cycle`]s each, plus reset
//! weeks, plus an optional bonus "week zero" on cyclic-leap
//! years.
//!
//! Deterministic — same inputs always produce the same output,
//! including UUIDs (derived via UUIDv5 from
//! `cycle:<year>:Q<n>:C<m>:<start>` so two machines agree
//! without coordinating).

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use uuid::Uuid;

use crate::CYCLE_NS;
use crate::model::{Cycle, Cycles, Quarter, weekday_to_short};

/// How to pick week 1 of a cyclic year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWeekRule {
    /// Week 1 = the first week (starting on
    /// `year_start_day`) that has at least 4 days falling
    /// within the new calendar year. ISO-8601 style.
    /// Cody's preferred convention.
    AtLeastFourDaysInYear,
    /// Week 1 = the week containing January 1. Closer to
    /// US-style "first week of the year". Calendar may start
    /// later than `AtLeastFourDaysInYear`.
    ContainsJan1,
}

/// Date on which week 1 of `year` starts, under the given
/// rule + week-start day.
#[must_use]
pub fn week_one_start(year: i32, year_start_day: Weekday, rule: FirstWeekRule) -> NaiveDate {
    let jan1 = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid jan 1");
    let monday_on_or_before_jan1 = step_back_to(jan1, year_start_day);
    match rule {
        FirstWeekRule::ContainsJan1 => monday_on_or_before_jan1,
        FirstWeekRule::AtLeastFourDaysInYear => {
            // The candidate week is the one starting on
            // `monday_on_or_before_jan1`. Count its days that
            // are >= jan1. If >= 4 it qualifies; else advance
            // by 7.
            let days_in_year = (monday_on_or_before_jan1 + Duration::days(6) - jan1).num_days() + 1;
            if days_in_year >= 4 {
                monday_on_or_before_jan1
            } else {
                monday_on_or_before_jan1 + Duration::days(7)
            }
        }
    }
}

/// True when the year-after-`year` would gain a bonus "week
/// zero" appended to `year`'s Q4. Concretely: when the gap
/// between week 1 of `year + 1` and the day after `year`'s Q4
/// reset week is ≥ 7 days.
#[must_use]
pub fn has_bonus_week(year: i32, year_start_day: Weekday, rule: FirstWeekRule) -> bool {
    let this_year_w1 = week_one_start(year, year_start_day, rule);
    let next_year_w1 = week_one_start(year + 1, year_start_day, rule);
    let q4_reset_end = this_year_w1 + Duration::days(52 * 7 - 1);
    let gap_days = (next_year_w1 - q4_reset_end).num_days() - 1;
    gap_days >= 7
}

/// Generate the full cyclic calendar for `year`. Returns four
/// quarters in calendar order; each carries its three cycles,
/// reset week, and (on Q4 in cyclic-leap years) the bonus
/// week.
#[must_use]
pub fn generate_year(year: i32, year_start_day: Weekday, rule: FirstWeekRule) -> Vec<Quarter> {
    let w1 = week_one_start(year, year_start_day, rule);
    let mut quarters = Vec::with_capacity(4);
    let mut week_cursor = w1;
    for q_ordinal in 1u8..=4 {
        let q_start = week_cursor;
        let mut cycles = Vec::with_capacity(3);
        for c_ordinal in 1u8..=3 {
            let c_start = week_cursor;
            let c_end = c_start + Duration::days(27);
            cycles.push(Cycle {
                id: deterministic_cycle_id(year, q_ordinal, c_ordinal, c_start),
                year,
                quarter: q_ordinal,
                ordinal: c_ordinal,
                start_date: c_start,
                end_date: c_end,
                week_start_day: weekday_to_short(year_start_day).to_string(),
            });
            week_cursor = c_end + Duration::days(1);
        }
        let reset_start = week_cursor;
        let reset_end = reset_start + Duration::days(6);
        week_cursor = reset_end + Duration::days(1);

        let (bonus_start, bonus_end, q_end) =
            if q_ordinal == 4 && has_bonus_week(year, year_start_day, rule) {
                let bs = week_cursor;
                let be = bs + Duration::days(6);
                week_cursor = be + Duration::days(1);
                (Some(bs), Some(be), be)
            } else {
                (None, None, reset_end)
            };

        quarters.push(Quarter {
            id: deterministic_quarter_id(year, q_ordinal, q_start),
            year,
            ordinal: q_ordinal,
            start_date: q_start,
            end_date: q_end,
            cycles: Cycles(cycles),
            reset_week_start: reset_start,
            reset_week_end: reset_end,
            bonus_week_start: bonus_start,
            bonus_week_end: bonus_end,
        });
    }
    quarters
}

/// Pick the cycle a specific date falls inside, walking the
/// generated calendar for `date.year()`. Returns `None` if the
/// date is during a reset / bonus week (between cycles).
#[must_use]
pub fn cycle_for_date(
    date: NaiveDate,
    year_start_day: Weekday,
    rule: FirstWeekRule,
) -> Option<Cycle> {
    // Date may belong to the previous or next cyclic year (a
    // year's W1 can start in late December of the previous
    // calendar year). Try a small window.
    for cyclic_year in [date.year() - 1, date.year(), date.year() + 1] {
        for q in generate_year(cyclic_year, year_start_day, rule) {
            for c in q.cycles.0 {
                if c.start_date <= date && date <= c.end_date {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// Convenience: the cycle that contains today's date in the
/// system's local timezone, or `None` if today is a reset /
/// bonus week.
#[must_use]
pub fn current_cycle(year_start_day: Weekday, rule: FirstWeekRule) -> Option<Cycle> {
    let today = chrono::Local::now().date_naive();
    cycle_for_date(today, year_start_day, rule)
}

/// Walk back to the most recent occurrence of `target_day`
/// (inclusive: returns `from` itself if `from`'s weekday
/// matches `target_day`).
fn step_back_to(from: NaiveDate, target_day: Weekday) -> NaiveDate {
    let mut d = from;
    while d.weekday() != target_day {
        d -= Duration::days(1);
    }
    d
}

fn deterministic_cycle_id(year: i32, quarter: u8, ordinal: u8, start_date: NaiveDate) -> Uuid {
    let name = format!("cycle:{year}:Q{quarter}:C{ordinal}:{start_date}");
    Uuid::new_v5(&CYCLE_NS, name.as_bytes())
}

fn deterministic_quarter_id(year: i32, quarter: u8, start_date: NaiveDate) -> Uuid {
    let name = format!("quarter:{year}:Q{quarter}:{start_date}");
    Uuid::new_v5(&CYCLE_NS, name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User-specified anchor: for Monday-start / ≥4-day rule,
    /// 2026 week 1 starts on Dec 29, 2025.
    #[test]
    fn week_one_2026_monday_at_least_four_days() {
        let w1 = week_one_start(2026, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);
        assert_eq!(w1, NaiveDate::from_ymd_opt(2025, 12, 29).unwrap());
    }

    /// 2025 Jan 1 is Wednesday → walking back to Monday lands
    /// on Dec 30, 2024. That week has 5 days in 2025 (Wed..Sun)
    /// so it qualifies under the ≥4-day rule.
    #[test]
    fn week_one_2025_monday_at_least_four_days() {
        let w1 = week_one_start(2025, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);
        assert_eq!(w1, NaiveDate::from_ymd_opt(2024, 12, 30).unwrap());
    }

    /// 2024 Jan 1 is Monday → that week itself is week 1
    /// (7 days in 2024, well over the threshold).
    #[test]
    fn week_one_2024_monday_at_least_four_days() {
        let w1 = week_one_start(2024, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);
        assert_eq!(w1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
    }

    /// User-specified: 2026 / 2032 / 2037 are cyclic-leap
    /// years for Mon-start / ≥4-day rule.
    #[test]
    fn cyclic_leap_years_match_user_spec() {
        assert!(has_bonus_week(
            2026,
            Weekday::Mon,
            FirstWeekRule::AtLeastFourDaysInYear
        ));
        assert!(has_bonus_week(
            2032,
            Weekday::Mon,
            FirstWeekRule::AtLeastFourDaysInYear
        ));
        assert!(has_bonus_week(
            2037,
            Weekday::Mon,
            FirstWeekRule::AtLeastFourDaysInYear
        ));
        // Counter-examples: ordinary years.
        assert!(!has_bonus_week(
            2025,
            Weekday::Mon,
            FirstWeekRule::AtLeastFourDaysInYear
        ));
        assert!(!has_bonus_week(
            2027,
            Weekday::Mon,
            FirstWeekRule::AtLeastFourDaysInYear
        ));
    }

    #[test]
    fn year_2026_shape_under_monday_rule() {
        let qs = generate_year(2026, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);
        assert_eq!(qs.len(), 4);
        for q in &qs {
            assert_eq!(q.cycles.len(), 3);
            // Each cycle is exactly 28 days.
            for c in q.cycles.iter() {
                assert_eq!((c.end_date - c.start_date).num_days(), 27);
                assert_eq!(c.start_date.weekday(), Weekday::Mon);
                assert_eq!(c.end_date.weekday(), Weekday::Sun);
            }
            // Reset week is exactly 7 days, starts Mon ends Sun.
            assert_eq!((q.reset_week_end - q.reset_week_start).num_days(), 6);
            assert_eq!(q.reset_week_start.weekday(), Weekday::Mon);
            assert_eq!(q.reset_week_end.weekday(), Weekday::Sun);
        }
        // Q4 of 2026 carries the bonus week.
        let q4 = &qs[3];
        assert!(q4.bonus_week_start.is_some());
        assert!(q4.bonus_week_end.is_some());
        // Q1.C1 starts at week-1.
        assert_eq!(
            qs[0].cycles[0].start_date,
            NaiveDate::from_ymd_opt(2025, 12, 29).unwrap()
        );
    }

    #[test]
    fn cycle_for_date_finds_2026_q1_c1() {
        // Jan 5, 2026 is in week 2 of cycle Q1.C1 (which started Dec 29, 2025).
        let date = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        let c = cycle_for_date(date, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear)
            .expect("date should land in a cycle");
        assert_eq!(c.year, 2026);
        assert_eq!(c.quarter, 1);
        assert_eq!(c.ordinal, 1);
    }

    #[test]
    fn cycle_for_date_returns_none_during_reset() {
        // 2026 Q1 reset week starts 13 weeks after Dec 29, 2025
        // → Dec 29 + 84 days (3 cycles × 28 days) = Mar 23, 2026.
        let qs = generate_year(2026, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);
        let reset_mid = qs[0].reset_week_start + Duration::days(3);
        assert!(
            cycle_for_date(
                reset_mid,
                Weekday::Mon,
                FirstWeekRule::AtLeastFourDaysInYear
            )
            .is_none()
        );
    }

    #[test]
    fn deterministic_ids_are_stable() {
        let a = generate_year(2026, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);
        let b = generate_year(2026, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);
        assert_eq!(a[0].id, b[0].id);
        assert_eq!(a[0].cycles[0].id, b[0].cycles[0].id);
    }

    #[test]
    fn sunday_start_works() {
        let w1 = week_one_start(2026, Weekday::Sun, FirstWeekRule::AtLeastFourDaysInYear);
        // 2026 Jan 1 is Thursday → walk back to Sunday Dec 28 2025;
        // that week has 4 days in 2026 (Thu Fri Sat plus Sun → wait, Sun Dec 28 + 6 days = Sat Jan 3, 2026; days in 2026 = Jan 1-3 = 3 days only). So the rule pushes to Jan 4.
        assert_eq!(w1, NaiveDate::from_ymd_opt(2026, 1, 4).unwrap());
        let qs = generate_year(2026, Weekday::Sun, FirstWeekRule::AtLeastFourDaysInYear);
        assert_eq!(qs[0].cycles[0].start_date.weekday(), Weekday::Sun);
        assert_eq!(qs[0].cycles[0].end_date.weekday(), Weekday::Sat);
    }
}
