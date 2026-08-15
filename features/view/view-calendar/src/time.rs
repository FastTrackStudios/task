//! Date / time layout helpers.
//!
//! All functions are pure and live in this module so the view
//! components stay focused on rendering. Week starts on Monday for
//! v1 — defer locale-driven Sunday-start until a settings layer
//! exists.

use chrono::{DateTime, Datelike, Days, Duration, NaiveDate, TimeZone, Utc, Weekday};

pub const WEEK_START: Weekday = Weekday::Mon;

/// The Monday of the ISO week containing `date`.
#[must_use]
pub fn week_start(date: NaiveDate) -> NaiveDate {
    let dow = date.weekday().num_days_from_monday();
    date - Days::new(u64::from(dow))
}

/// Seven consecutive dates starting at `week_start(date)`.
#[must_use]
pub fn week_days(date: NaiveDate) -> [NaiveDate; 7] {
    let start = week_start(date);
    let mut out = [start; 7];
    for (i, d) in out.iter_mut().enumerate() {
        *d = start + Days::new(i as u64);
    }
    out
}

/// 6×7 month grid containing `date.month()`. The grid is padded
/// with trailing days of the previous month and leading days of the
/// next month so each row has seven cells and the grid always has
/// six rows (matching Google's layout — keeps the chart height
/// stable when paging between months).
#[must_use]
pub fn month_grid(date: NaiveDate) -> [[NaiveDate; 7]; 6] {
    let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("valid month");
    let grid_start = week_start(first);
    let mut out = [[grid_start; 7]; 6];
    // 6×7 fixed grid; we want (row, col) indices for the
    // arithmetic, so the index loop is the natural shape.
    #[allow(clippy::needless_range_loop)]
    for row in 0..6 {
        for col in 0..7 {
            out[row][col] = grid_start + Days::new((row * 7 + col) as u64);
        }
    }
    out
}

/// Midnight UTC for `date`. Cheap convenience that keeps the
/// timezone hand-wave (v1 == "treat user time as UTC") out of the
/// caller's hair.
#[must_use]
pub fn day_start_utc(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight"))
}

/// Exclusive day end (midnight UTC of the next day).
#[must_use]
pub fn day_end_utc(date: NaiveDate) -> DateTime<Utc> {
    day_start_utc(date) + Duration::days(1)
}

/// Place a part-day event on a 24-hour time grid. Returns
/// `(top_pct, height_pct)` clamped to `[0, 100]`. Events that span
/// midnight will clip at the day edge — multi-day rendering is a
/// month-view concern, not a time-grid one.
///
/// `day` is the date whose grid row the event lives in.
#[must_use]
pub fn position_in_day(day: NaiveDate, start: DateTime<Utc>, end: DateTime<Utc>) -> (f32, f32) {
    let day_start = day_start_utc(day);
    let day_end = day_end_utc(day);
    let clipped_start = start.max(day_start);
    let clipped_end = end.min(day_end);
    let total = (day_end - day_start).num_seconds() as f32;
    let top = (clipped_start - day_start).num_seconds() as f32 / total * 100.0;
    let bot = (clipped_end - day_start).num_seconds() as f32 / total * 100.0;
    let height = (bot - top).max(0.5);
    (top.clamp(0.0, 100.0), height.clamp(0.5, 100.0))
}

/// Hour labels for the time-grid axis (e.g. `"1 AM"`, `"12 PM"`).
/// Returns 24 entries skipping the 0th (drawn as just the day
/// header) — matches Google's display.
#[must_use]
pub fn hour_labels() -> Vec<(u32, String)> {
    (1..24)
        .map(|h| {
            let label = match h {
                0 => "12 AM".to_string(),
                1..=11 => format!("{h} AM"),
                12 => "12 PM".to_string(),
                _ => format!("{} PM", h - 12),
            };
            (h, label)
        })
        .collect()
}

/// Shift `start` and `end` by `delta` days, keeping duration.
#[must_use]
pub fn shift_days(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    delta_days: i64,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let d = Duration::days(delta_days);
    (start + d, end + d)
}
