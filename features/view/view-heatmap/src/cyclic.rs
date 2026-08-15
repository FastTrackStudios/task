//! Cyclic planning calendar.
//!
//! Splits a year into **4 quarters × 13 weeks** where each quarter
//! is **3 cycles of 4 weeks + 1 reset week**. Total = 52 weeks =
//! 364 days. A cyclic leap year (≈ every 5–6 years) gets a bonus
//! 53rd week the user can treat as "week 0" for the next year.
//!
//! Year 1 starts on the first instance of `week_start` (Monday by
//! default) where the week contains ≥ 4 days of the calendar year
//! — same rule ISO 8601 uses for week 1, so the cyclic year hugs
//! the calendar year without ever drifting more than ~3 days.
//!
//! Pure math + types, no Dioxus.

use chrono::{Datelike, Days, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

/// Number of regular weeks in a cyclic year (excludes the bonus
/// week — 4 quarters × 13 weeks = 52).
pub const WEEKS_PER_YEAR: u32 = 52;
/// Weeks per quarter — 3 cycles × 4 weeks + reset week.
pub const WEEKS_PER_QUARTER: u32 = 13;
/// Weeks per cycle.
pub const WEEKS_PER_CYCLE: u32 = 4;
/// Cycles per quarter (before the reset week).
pub const CYCLES_PER_QUARTER: u32 = 3;

/// What occupies week `w` inside a quarter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeekSlot {
    /// Cycle `cycle` (0-indexed, 0..=2), week `week_in_cycle` (0..=3).
    Cycle { cycle: u8, week_in_cycle: u8 },
    /// Reset week — always week 13 of the quarter.
    Reset,
    /// Bonus "week 0" for the next year. Only emitted when
    /// `CyclicConfig::bonus_week` is set.
    Bonus,
}

/// Resolved coordinate of one week.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeekCoord {
    /// 0..=3 (Q1 .. Q4). Bonus weeks return `quarter = 4` for
    /// rendering convenience.
    pub quarter: u8,
    pub slot: WeekSlot,
    pub start: NaiveDate,
}

/// Config for one cyclic year.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CyclicConfig {
    /// Calendar year this cyclic year maps to.
    pub year: i32,
    /// Date of day 1 of week 1.
    pub year_start: NaiveDate,
    /// Weekday all weeks begin on.
    pub week_start: Weekday,
    /// True if this is a cyclic leap year — gets a 53rd "week 0"
    /// at the end. Auto-computed by `for_year`; can be overridden
    /// for testing.
    pub bonus_week: bool,
}

impl CyclicConfig {
    /// Build the config for `year` with the given week-start day.
    /// Mirrors the rule the source video describes: week 1 is the
    /// first `week_start` whose 7-day window contains ≥ 4 days of
    /// `year`.
    #[must_use]
    pub fn for_year(year: i32, week_start: Weekday) -> Self {
        let year_start = first_week_start(year, week_start);
        let next_year_start = first_week_start(year + 1, week_start);
        let gap = (next_year_start - year_start).num_days();
        let bonus_week = gap >= 53 * 7;
        Self {
            year,
            year_start,
            week_start,
            bonus_week,
        }
    }

    /// Total weeks rendered for this year (52 or 53).
    #[must_use]
    pub fn total_weeks(self) -> u32 {
        if self.bonus_week { 53 } else { 52 }
    }

    /// Iterate every week in the year in display order. Indexes
    /// 0..52 are the regular weeks; index 52 (if present) is the
    /// bonus week.
    pub fn weeks(self) -> impl Iterator<Item = WeekCoord> {
        let start = self.year_start;
        let bonus = self.bonus_week;
        (0..self.total_weeks()).map(move |i| {
            let week_start = start + Days::new(u64::from(i) * 7);
            if bonus && i == 52 {
                WeekCoord {
                    quarter: 4,
                    slot: WeekSlot::Bonus,
                    start: week_start,
                }
            } else {
                let quarter = (i / WEEKS_PER_QUARTER) as u8;
                let week_in_quarter = (i % WEEKS_PER_QUARTER) as u8;
                let slot = if week_in_quarter == 12 {
                    WeekSlot::Reset
                } else {
                    WeekSlot::Cycle {
                        cycle: week_in_quarter / 4,
                        week_in_cycle: week_in_quarter % 4,
                    }
                };
                WeekCoord {
                    quarter,
                    slot,
                    start: week_start,
                }
            }
        })
    }
}

/// First `week_start` date with ≥ 4 days inside `year`. ISO-8601
/// week-1 rule, applied to an arbitrary first day-of-week.
fn first_week_start(year: i32, week_start: Weekday) -> NaiveDate {
    let jan1 = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid jan 1");
    let dow_diff = i64::from(jan1.weekday().num_days_from_monday())
        - i64::from(week_start.num_days_from_monday());
    let dow_diff = ((dow_diff % 7) + 7) % 7; // 0..7
    let candidate = jan1 - Days::new(dow_diff as u64);
    let days_in_year = (0..7)
        .filter(|i| (candidate + Days::new(*i as u64)).year() == year)
        .count();
    if days_in_year >= 4 {
        candidate
    } else {
        candidate + Days::new(7)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_week_2026_monday_start() {
        // Jan 1 2026 is a Thursday; week Mon Dec 29 → Sun Jan 4
        // has 4 days in 2026 (Jan 1, 2, 3, 4) → that's week 1.
        let d = first_week_start(2026, Weekday::Mon);
        assert_eq!(d, NaiveDate::from_ymd_opt(2025, 12, 29).unwrap());
    }

    #[test]
    fn first_week_2027_monday_start() {
        // Jan 1 2027 is Friday; week Mon Dec 28 → Sun Jan 3 has
        // only 3 days in 2027 → skip to next Monday Jan 4.
        let d = first_week_start(2027, Weekday::Mon);
        assert_eq!(d, NaiveDate::from_ymd_opt(2027, 1, 4).unwrap());
    }

    #[test]
    fn cyclic_leap_year_2026_has_bonus_week() {
        let c = CyclicConfig::for_year(2026, Weekday::Mon);
        assert!(c.bonus_week);
        assert_eq!(c.total_weeks(), 53);
    }

    #[test]
    fn ordinary_year_2027_has_no_bonus_week() {
        let c = CyclicConfig::for_year(2027, Weekday::Mon);
        assert!(!c.bonus_week);
        assert_eq!(c.total_weeks(), 52);
    }

    #[test]
    fn quarter_and_slot_layout_2026() {
        let c = CyclicConfig::for_year(2026, Weekday::Mon);
        let weeks: Vec<_> = c.weeks().collect();
        // 53 weeks: 52 regular + 1 bonus
        assert_eq!(weeks.len(), 53);

        // Week 0 (index 0) — Q1, Cycle 0, Week 0.
        assert_eq!(weeks[0].quarter, 0);
        assert!(matches!(
            weeks[0].slot,
            WeekSlot::Cycle {
                cycle: 0,
                week_in_cycle: 0
            }
        ));
        // Week 12 (index 12) — Q1 reset week.
        assert_eq!(weeks[12].quarter, 0);
        assert_eq!(weeks[12].slot, WeekSlot::Reset);
        // Week 13 (index 13) — Q2, Cycle 0, Week 0.
        assert_eq!(weeks[13].quarter, 1);
        assert!(matches!(
            weeks[13].slot,
            WeekSlot::Cycle {
                cycle: 0,
                week_in_cycle: 0
            }
        ));
        // Week 51 — Q4 reset week.
        assert_eq!(weeks[51].quarter, 3);
        assert_eq!(weeks[51].slot, WeekSlot::Reset);
        // Bonus week.
        assert_eq!(weeks[52].quarter, 4);
        assert_eq!(weeks[52].slot, WeekSlot::Bonus);
    }
}
