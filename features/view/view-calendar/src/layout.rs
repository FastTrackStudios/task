//! Layout pre-passes shared by the month and time-grid views.
//!
//! Pure functions — no Dioxus. The views call into here to turn a
//! flat event list into positioned placements.
//!
//! Two passes:
//!
//! - [`month_week_layout`] groups events into "tracks" per week row
//!   so multi-day events render as a single connected chip spanning
//!   day columns. Chips wrap at week boundaries with
//!   `continues_left` / `continues_right` flags so the chrome can
//!   square off the cut edge.
//! - [`day_overlap_layout`] clusters transitively-overlapping
//!   events on a single day and assigns each event a sub-column
//!   (`FullCalendar` / Google Calendar's algorithm). The view turns
//!   the `column` + `cluster_size` pair into `left%` / `width%`.

use chrono::{Days, NaiveDate};

use crate::time::{day_end_utc, day_start_utc};
use crate::types::CalendarEvent;

/// One event's placement inside a horizontal day-window (a month
/// week row, or the all-day strip above a week / day time grid).
/// Coordinates are in the local `0..window_len` day-column frame;
/// the renderer turns them into CSS `grid-column: X / span Y`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipPlacement {
    pub event: CalendarEvent,
    /// `0..window_len` — first day column the chip occupies.
    pub start_col: u8,
    /// `1..=window_len` — number of day columns spanned.
    pub span: u8,
    /// 0..N — vertical stacking order ("track") in the row.
    pub track: u8,
    /// Event actually started before this window — the renderer
    /// should drop the left rounded corner so the pill looks
    /// continuous.
    pub continues_left: bool,
    /// Event continues past this window.
    pub continues_right: bool,
}

/// Backwards-compatible alias for callers that want the month-week
/// shape (now unified with the all-day-strip shape).
pub type MonthChipPlacement = ChipPlacement;

/// Greedy-track lay out events across a horizontal day-window
/// starting at `start` and spanning `len` days. Used for both the
/// month-view week rows (`len = 7`) and the time-grid all-day
/// strip (`len = days.len()` for week or day view).
///
/// Sorted by (track, `start_col`) so the renderer can iterate in
/// display order.
#[must_use]
pub fn track_layout(start: NaiveDate, len: u8, events: &[CalendarEvent]) -> Vec<ChipPlacement> {
    assert!(len >= 1, "window must be non-empty");
    let end = start + Days::new(u64::from(len));
    let window_start_utc = day_start_utc(start);
    let window_end_utc = day_start_utc(end);

    let mut hits: Vec<&CalendarEvent> = events
        .iter()
        .filter(|ev| ev.end > window_start_utc && ev.start < window_end_utc)
        .collect();
    hits.sort_by_key(|ev| (ev.start, std::cmp::Reverse(ev.duration_minutes())));

    let mut tracks: Vec<u8> = Vec::new();
    let mut out: Vec<ChipPlacement> = Vec::with_capacity(hits.len());

    for ev in hits {
        let start_day = ev.start.date_naive().max(start);
        let last_visible = (ev.end - chrono::Duration::minutes(1))
            .date_naive()
            .min(end - Days::new(1));
        let start_col = (start_day - start).num_days() as u8;
        let last_col = (last_visible - start).num_days() as u8;
        let span = last_col.saturating_sub(start_col) + 1;

        let track = if let Some(i) = tracks.iter().position(|&right| right <= start_col) {
            tracks[i] = start_col + span;
            i as u8
        } else {
            tracks.push(start_col + span);
            (tracks.len() - 1) as u8
        };

        out.push(ChipPlacement {
            event: ev.clone(),
            start_col,
            span,
            track,
            continues_left: ev.start < window_start_utc,
            continues_right: ev.end > window_end_utc,
        });
    }

    out.sort_by_key(|p| (p.track, p.start_col));
    out
}

/// Convenience wrapper: month view always lays out 7-day rows.
#[must_use]
pub fn month_week_layout(week_start: NaiveDate, events: &[CalendarEvent]) -> Vec<ChipPlacement> {
    track_layout(week_start, 7, events)
}

/// One event's placement on a single day of the time grid. The view
/// turns `(column, cluster_size)` into `left: %; width: %;`.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeBlockPlacement {
    pub event: CalendarEvent,
    /// `0..cluster_size` — which sub-column the event occupies.
    pub column: u8,
    /// Total sub-columns in the cluster the event belongs to.
    /// Width of each is `100 / cluster_size %`.
    pub cluster_size: u8,
    /// Top offset within the day, in minutes from 00:00.
    pub top_min: i64,
    /// Block height in minutes. Always ≥ 15 so very-short events
    /// stay clickable.
    pub height_min: i64,
}

/// Cluster + column-assign overlapping events on `day`.
///
/// Algorithm (`FullCalendar` / Google):
/// 1. Sort events by `start`, then by descending duration so
///    longer events land in the leftmost columns.
/// 2. Walk events; an event joins the active cluster if it starts
///    before the cluster's running `max_end`.
/// 3. Inside a cluster, assign each event the smallest column index
///    not occupied by any still-active overlapper (`other.end >
///    new.start`).
/// 4. When the cluster closes (`new.start >= max_end`), commit
///    `cluster_size = max(col) + 1` to every event in it.
#[must_use]
pub fn day_overlap_layout(day: NaiveDate, events: &[CalendarEvent]) -> Vec<TimeBlockPlacement> {
    let day_start = day_start_utc(day);
    let day_end = day_end_utc(day);

    // Filter + clip to the day.
    let mut hits: Vec<CalendarEvent> = events
        .iter()
        .filter(|ev| ev.end > day_start && ev.start < day_end)
        .cloned()
        .collect();
    hits.sort_by_key(|ev| (ev.start, std::cmp::Reverse(ev.duration_minutes())));

    let mut out: Vec<TimeBlockPlacement> = Vec::with_capacity(hits.len());

    // Running cluster state.
    let mut cluster_start_idx = 0usize;
    let mut cluster_max_end = chrono::DateTime::<chrono::Utc>::MIN_UTC;
    let mut active_cols: Vec<chrono::DateTime<chrono::Utc>> = Vec::new();

    let commit_cluster = |out: &mut Vec<TimeBlockPlacement>, start_idx: usize| {
        if out.is_empty() {
            return;
        }
        let max_col = out[start_idx..].iter().map(|p| p.column).max().unwrap_or(0);
        let size = max_col + 1;
        for p in &mut out[start_idx..] {
            p.cluster_size = size;
        }
    };

    for ev in hits {
        // New cluster?
        if ev.start >= cluster_max_end && !out.is_empty() {
            commit_cluster(&mut out, cluster_start_idx);
            cluster_start_idx = out.len();
            active_cols.clear();
            cluster_max_end = chrono::DateTime::<chrono::Utc>::MIN_UTC;
        }

        // Pick the lowest column whose existing occupant has ended.
        let column = if let Some(i) = active_cols.iter().position(|end| *end <= ev.start) {
            active_cols[i] = ev.end;
            i as u8
        } else {
            active_cols.push(ev.end);
            (active_cols.len() - 1) as u8
        };

        if ev.end > cluster_max_end {
            cluster_max_end = ev.end;
        }

        let clipped_start = ev.start.max(day_start);
        let clipped_end = ev.end.min(day_end);
        let top_min = (clipped_start - day_start).num_minutes().max(0);
        let height_min = (clipped_end - clipped_start).num_minutes().max(15);

        out.push(TimeBlockPlacement {
            event: ev,
            column,
            cluster_size: 1, // patched at cluster commit
            top_min,
            height_min,
        });
    }

    commit_cluster(&mut out, cluster_start_idx);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn ev(title: &str, day: NaiveDate, hours: (i64, i64)) -> CalendarEvent {
        let s = day_start_utc(day) + chrono::Duration::hours(hours.0);
        let e = day_start_utc(day) + chrono::Duration::hours(hours.1);
        CalendarEvent::new(title, s, e)
    }

    #[test]
    fn overlap_two_back_to_back_share_a_column() {
        let d = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        let a = ev("A", d, (9, 10));
        let b = ev("B", d, (10, 11));
        let placed = day_overlap_layout(d, &[a, b]);
        assert_eq!(placed.len(), 2);
        assert!(placed.iter().all(|p| p.column == 0 && p.cluster_size == 1));
    }

    #[test]
    fn overlap_two_concurrent_split_to_two_columns() {
        let d = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        let a = ev("A", d, (9, 11));
        let b = ev("B", d, (10, 12));
        let placed = day_overlap_layout(d, &[a, b]);
        assert_eq!(placed.len(), 2);
        assert!(placed.iter().all(|p| p.cluster_size == 2));
        assert_eq!(placed[0].column, 0);
        assert_eq!(placed[1].column, 1);
    }

    #[test]
    fn month_layout_multi_day_spans_three_columns() {
        let week = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(); // Monday
        let start = Utc.from_utc_datetime(&week.and_hms_opt(9, 0, 0).unwrap());
        let end = Utc.from_utc_datetime(&(week + Days::new(2)).and_hms_opt(10, 0, 0).unwrap());
        let e = CalendarEvent::new("Trip", start, end);
        let placed = month_week_layout(week, &[e]);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].start_col, 0);
        assert_eq!(placed[0].span, 3);
        assert!(!placed[0].continues_left);
        assert!(!placed[0].continues_right);
    }

    #[test]
    fn month_layout_wraps_at_week_edge() {
        // Event Sun → Tue spans two weeks; in the first week it
        // occupies col 6 alone and continues right; in the second
        // week it occupies cols 0..2 with continues_left.
        let week1 = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();
        let sun = week1 + Days::new(6);
        let start = day_start_utc(sun);
        let end = day_start_utc(sun + Days::new(2)) + chrono::Duration::hours(12);
        let e = CalendarEvent::new("Trip", start, end);

        let week1_layout = month_week_layout(week1, std::slice::from_ref(&e));
        assert_eq!(week1_layout[0].start_col, 6);
        assert_eq!(week1_layout[0].span, 1);
        assert!(week1_layout[0].continues_right);

        let week2 = week1 + Days::new(7);
        let week2_layout = month_week_layout(week2, &[e]);
        assert_eq!(week2_layout[0].start_col, 0);
        assert_eq!(week2_layout[0].span, 2);
        assert!(week2_layout[0].continues_left);
    }
}
