//! Date-math helpers — port of svar `store/src/time.ts`.
//!
//! All times in UTC. `unit_start` snaps a date to the start of its
//! containing unit (e.g. start-of-week for `Week`). `add` and `diff`
//! work in whole units; fractional cell positions are derived by
//! mixing diff at the parent unit with a sub-unit ratio in `scales`.

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};

use crate::types::LengthUnit;

/// Day of week used as start-of-week. Monday = 1, Sunday = 0.
/// (chrono `Weekday::num_days_from_monday` returns Mon=0; we keep
/// 0-indexed-from-monday internally too.)
pub type WeekStart = u8;

pub const MONDAY_START: WeekStart = 0;

/// Snap `date` to the start of the unit it belongs to.
#[must_use]
pub fn unit_start(unit: LengthUnit, date: DateTime<Utc>, week_start: WeekStart) -> DateTime<Utc> {
    use LengthUnit::{Day, Hour, Minute, Month, Quarter, Week, Year};
    match unit {
        Minute => date
            .with_second(0)
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(date),
        Hour => date
            .with_minute(0)
            .and_then(|d| d.with_second(0))
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(date),
        Day => Utc
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
            .single()
            .unwrap_or(date),
        Week => {
            let day = Utc
                .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
                .single()
                .unwrap_or(date);
            // 0-indexed-from-monday.
            let dow = i64::from(day.weekday().num_days_from_monday());
            let target = i64::from(week_start);
            let shift = (dow - target).rem_euclid(7);
            day - Duration::days(shift)
        }
        Month => Utc
            .with_ymd_and_hms(date.year(), date.month(), 1, 0, 0, 0)
            .single()
            .unwrap_or(date),
        Quarter => {
            let q_month = ((date.month() - 1) / 3) * 3 + 1;
            Utc.with_ymd_and_hms(date.year(), q_month, 1, 0, 0, 0)
                .single()
                .unwrap_or(date)
        }
        Year => Utc
            .with_ymd_and_hms(date.year(), 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(date),
    }
}

/// Add `n` units to `date`. Negative values subtract.
#[must_use]
pub fn add(unit: LengthUnit, date: DateTime<Utc>, n: i64) -> DateTime<Utc> {
    use LengthUnit::{Day, Hour, Minute, Month, Quarter, Week, Year};
    match unit {
        Minute => date + Duration::minutes(n),
        Hour => date + Duration::hours(n),
        Day => date + Duration::days(n),
        Week => date + Duration::weeks(n),
        Month => add_months(date, n),
        Quarter => add_months(date, n * 3),
        Year => {
            let y = i64::from(date.year()) + n;
            date.with_year(y as i32).unwrap_or(date)
        }
    }
}

fn add_months(date: DateTime<Utc>, n: i64) -> DateTime<Utc> {
    let total = i64::from(date.year()) * 12 + (i64::from(date.month()) - 1) + n;
    let new_year = total.div_euclid(12) as i32;
    let new_month = total.rem_euclid(12) as u32 + 1;
    let day = clamp_day(new_year, new_month, date.day());
    Utc.with_ymd_and_hms(
        new_year,
        new_month,
        day,
        date.hour(),
        date.minute(),
        date.second(),
    )
    .single()
    .unwrap_or(date)
}

fn clamp_day(year: i32, month: u32, day: u32) -> u32 {
    let last = last_day_of_month(year, month);
    day.min(last)
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = Utc
        .with_ymd_and_hms(ny, nm, 1, 0, 0, 0)
        .single()
        .expect("valid first-of-month");
    (first_next - Duration::days(1)).day()
}

/// Difference `a - b` in whole units (fractional via `diff_f`).
#[must_use]
pub fn diff(unit: LengthUnit, a: DateTime<Utc>, b: DateTime<Utc>) -> i64 {
    diff_f(unit, a, b) as i64
}

/// Fractional diff — number of `unit`s from `b` to `a`.
///
/// For month/quarter/year the integer part comes from real calendar
/// arithmetic (`a.year*12 + a.month - …`); the fractional part is
/// measured against the actual length of the month `b` falls in,
/// which is what svar's `lib-schedule` does. This avoids the 1.0%
/// drift the prior 30-day approximation had near long months.
#[must_use]
pub fn diff_f(unit: LengthUnit, a: DateTime<Utc>, b: DateTime<Utc>) -> f64 {
    use LengthUnit::{Day, Hour, Minute, Month, Quarter, Week, Year};
    let secs = (a - b).num_milliseconds() as f64 / 1000.0;
    match unit {
        Minute => secs / 60.0,
        Hour => secs / 3600.0,
        Day => secs / 86_400.0,
        Week => secs / (86_400.0 * 7.0),
        Month => calendar_month_diff(a, b),
        Quarter => calendar_month_diff(a, b) / 3.0,
        Year => f64::from(a.year() - b.year()) + calendar_month_diff(a, b) / 12.0,
    }
}

fn calendar_month_diff(a: DateTime<Utc>, b: DateTime<Utc>) -> f64 {
    let whole = (i64::from(a.year()) - i64::from(b.year())) * 12
        + (i64::from(a.month()) - i64::from(b.month()));
    // Fractional remainder is `(a - first-of-a's-month) / days-in-a's-month`
    // minus the same for b. Each ratio is the position-within-month
    // in [0, 1).
    let frac_a = month_fraction(a);
    let frac_b = month_fraction(b);
    whole as f64 + frac_a - frac_b
}

fn month_fraction(d: DateTime<Utc>) -> f64 {
    let days_in_month = f64::from(last_day_of_month(d.year(), d.month()));
    let seconds_into_month = (f64::from(d.day()) - 1.0) * 86_400.0
        + f64::from(d.hour()) * 3600.0
        + f64::from(d.minute()) * 60.0
        + f64::from(d.second());
    seconds_into_month / (days_in_month * 86_400.0)
}

/// Approximate width-in-pixels of one `unit` cell given the chart's
/// base cell width (the *min* unit width).
#[must_use]
pub fn unit_pixel_width(unit: LengthUnit, min_unit: LengthUnit, cell_width: f32) -> f32 {
    let sample = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    let next = add(unit, sample, 1);
    diff_f(min_unit, next, sample) as f32 * cell_width
}

/// Pick the smallest-granularity unit from a scale list.
#[must_use]
pub fn min_unit(units: &[LengthUnit]) -> LengthUnit {
    units
        .iter()
        .copied()
        .min_by_key(|u| u.rank())
        .unwrap_or(LengthUnit::Day)
}
