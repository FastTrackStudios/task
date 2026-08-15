//! Cycle-aware visual overlays for calendar views.
//!
//! Resolves a date against the cyclic-life-calendar
//! generator and returns enough info to tint day cells.
//! Reset weeks + bonus weeks get a muted background so the
//! weekly rhythm is obvious at a glance.

use chrono::{Datelike, NaiveDate, Weekday};
use cycle::{FirstWeekRule, generate_year};

/// What kind of period a date sits inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateKind {
    /// Inside one of the three 4-week cycles of its quarter.
    InCycle,
    /// 13th week of a quarter — the reset week.
    ResetWeek,
    /// Bonus "week zero" appended to Q4 on cyclic-leap years.
    BonusWeek,
}

/// Classify `date` against Cody's default cycle config
/// (Monday-start, ≥4-day rule). User-prefs hook lands later.
#[must_use]
pub fn kind_for_date(date: NaiveDate) -> DateKind {
    let rule = FirstWeekRule::AtLeastFourDaysInYear;
    // Date may belong to the previous or next cyclic year (a
    // year's W1 can start in late December of the previous
    // calendar year, and bonus weeks straddle the boundary).
    for cyclic_year in [date.year() - 1, date.year(), date.year() + 1] {
        for q in generate_year(cyclic_year, Weekday::Mon, rule) {
            for c in q.cycles.iter() {
                if c.start_date <= date && date <= c.end_date {
                    return DateKind::InCycle;
                }
            }
            if date >= q.reset_week_start && date <= q.reset_week_end {
                return DateKind::ResetWeek;
            }
            if let (Some(s), Some(e)) = (q.bonus_week_start, q.bonus_week_end) {
                if date >= s && date <= e {
                    return DateKind::BonusWeek;
                }
            }
        }
    }
    DateKind::InCycle
}

/// Tailwind class fragment to apply to a day cell based on
/// its [`DateKind`]. Empty string for `InCycle` so callers
/// can blindly concatenate without producing unwanted styles.
#[must_use]
pub fn cell_tint_class(kind: DateKind) -> &'static str {
    match kind {
        DateKind::InCycle => "",
        // Reset week — soft warm tint, like a "rest stop" pull-out.
        DateKind::ResetWeek => "bg-muted/30",
        // Bonus / week-zero — accent tint, distinct from reset.
        DateKind::BonusWeek => "bg-accent/20",
    }
}

/// Short label for screen readers / hover tooltips on tinted
/// cells. `None` for `InCycle`.
#[must_use]
pub fn cell_tint_label(kind: DateKind) -> Option<&'static str> {
    match kind {
        DateKind::InCycle => None,
        DateKind::ResetWeek => Some("reset week"),
        DateKind::BonusWeek => Some("bonus · week zero"),
    }
}
