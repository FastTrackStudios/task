//! Spaced-repetition scheduling for [`InboxItem`], a faithful port of
//! the obsidian-spaced-repetition plugin's SM-2 `schedule()`
//! (github.com/st3v3nmw/obsidian-spaced-repetition, `src/scheduling.ts`).
//!
//! The inbox's daily "temporal contract" *is* a review: each open item
//! resurfaces on its due date, you grade how well it held up, and the
//! next due date is pushed out by the SM-2 interval. The UI's three
//! review buttons map onto the plugin's responses:
//!
//! | UI label  | [`ReviewResponse`] | plugin       |
//! |-----------|--------------------|--------------|
//! | Urgent    | `Hard`             | again/hard   |
//! | Maybe     | `Good`             | good         |
//! | Someday   | `Easy`             | easy         |
//!
//! State lives on the item: [`InboxItem::ease`] (`sr-ease`, ×100),
//! [`InboxItem::interval`] (`sr-interval`, days), [`InboxItem::reviews`]
//! (`sr-reviews`), and [`InboxItem::resurface_on`] as the due date
//! (`sr-due`).

use chrono::NaiveDate;

use crate::InboxItem;

/// Starting ease factor ×100 (plugin `baseEase`, 2.5).
pub const BASE_EASE: i64 = 250;
/// Extra interval multiplier applied to an `Easy` response (plugin
/// `easyBonus`).
pub const EASY_BONUS: f64 = 1.3;
/// Interval multiplier applied to a lapse / `Hard` response (plugin
/// `lapsesIntervalChange`).
pub const LAPSES_INTERVAL_CHANGE: f64 = 0.5;
/// Floor on the ease factor ×100 (plugin `minimumEase`, 1.3).
pub const MIN_EASE: i64 = 130;
/// Ceiling on the interval in days (plugin `maximumInterval`, 100 years).
pub const MAX_INTERVAL: i64 = 36525;

/// How well a resurfaced inbox item held up on review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewResponse {
    /// Struggled — shorten the interval and lower the ease (UI: Urgent).
    Hard,
    /// Recalled fine — grow the interval by the ease (UI: Maybe).
    Good,
    /// Trivially easy — grow the interval and raise the ease (UI: Someday).
    Easy,
}

/// The pure SM-2 step, a direct port of the plugin's `schedule()`.
///
/// * `interval_days` — the item's current interval; `0` (a brand-new
///   item) is treated as `1` day.
/// * `ease` — the current ease ×100; `0` is treated as [`BASE_EASE`].
/// * `delay_days` — how many days *late* the review is
///   (`max(0, today - due)`), `0` for an on-time or brand-new item.
///
/// Returns `(new_interval_days, new_ease)` with the interval rounded to
/// a whole day, floored at `1`, and capped at [`MAX_INTERVAL`].
#[must_use]
pub fn schedule(
    response: ReviewResponse,
    interval_days: i64,
    ease: i64,
    delay_days: i64,
) -> (i64, i64) {
    let mut e = if ease == 0 { BASE_EASE } else { ease };
    let mut i = if interval_days == 0 {
        1.0
    } else {
        interval_days as f64
    };
    let delay = delay_days.max(0) as f64;

    match response {
        ReviewResponse::Easy => {
            e += 20;
            i = (i + delay) * (e as f64 / 100.0) * EASY_BONUS;
        }
        ReviewResponse::Good => {
            i = (i + delay / 2.0) * (e as f64 / 100.0);
        }
        ReviewResponse::Hard => {
            e = MIN_EASE.max(e - 20);
            i = 1.0_f64.max((i + delay / 4.0) * LAPSES_INTERVAL_CHANGE);
        }
    }

    i = i.min(MAX_INTERVAL as f64).round().max(1.0);
    (i as i64, e)
}

/// The next spaced-repetition state for `item` reviewed on `today`.
///
/// Reads the item's current `interval` / `ease` / `resurface_on` (the
/// due date), computes the lateness delay (`max(0, today - due)`; `0`
/// for a never-scheduled item), runs [`schedule`], and returns
/// everything the caller writes back before an `upsert_inbox_item`:
///
/// `(new_interval_days, new_ease, new_resurface_on /* today + interval, ISO */, new_reviews)`.
///
/// Apply it with, e.g.:
/// ```ignore
/// let (interval, ease, due, reviews) = review(&item, resp, today);
/// item.interval = interval;
/// item.ease = ease;
/// item.resurface_on = Some(due);
/// item.reviews = reviews;
/// ```
#[must_use]
pub fn review(
    item: &InboxItem,
    response: ReviewResponse,
    today: NaiveDate,
) -> (i64, i64, String, i64) {
    let delay_days = item
        .resurface_on
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(&d[..d.len().min(10)], "%Y-%m-%d").ok())
        .map_or(0, |due| (today - due).num_days().max(0));

    let (interval, ease) = schedule(response, item.interval, item.ease, delay_days);
    let due = (today + chrono::Duration::days(interval)).to_string();
    (interval, ease, due, item.reviews + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_item_good_grows_by_base_ease() {
        // fresh item: interval 0 -> 1, ease 0 -> BASE_EASE(250).
        // Good on-time: (1 + 0) * 2.5 = 2.5 -> round 3? -> 2.5.round() = 3.
        let (i, e) = schedule(ReviewResponse::Good, 0, 0, 0);
        assert_eq!(e, 250);
        assert_eq!(i, 3); // 2.5 rounds to 3 (round half away from zero)
    }

    #[test]
    fn easy_raises_ease_and_applies_bonus() {
        // interval 10, ease 250, on-time, Easy:
        // e = 270; i = 10 * 2.7 * 1.3 = 35.1 -> 35.
        let (i, e) = schedule(ReviewResponse::Easy, 10, 250, 0);
        assert_eq!(e, 270);
        assert_eq!(i, 35);
    }

    #[test]
    fn hard_lowers_ease_floored_and_halves_interval() {
        // interval 10, ease 250, on-time, Hard:
        // e = 230; i = max(1, 10 * 0.5) = 5.
        let (i, e) = schedule(ReviewResponse::Hard, 10, 250, 0);
        assert_eq!(e, 230);
        assert_eq!(i, 5);
    }

    #[test]
    fn hard_floors_ease_at_min() {
        let (_i, e) = schedule(ReviewResponse::Hard, 10, MIN_EASE, 0);
        assert_eq!(e, MIN_EASE);
    }

    #[test]
    fn interval_capped_at_max() {
        let (i, _e) = schedule(ReviewResponse::Easy, MAX_INTERVAL, 300, 0);
        assert_eq!(i, MAX_INTERVAL);
    }

    #[test]
    fn delay_folds_into_good() {
        // interval 10, ease 250, 4 days late, Good:
        // i = (10 + 4/2) * 2.5 = 12 * 2.5 = 30.
        let (i, _e) = schedule(ReviewResponse::Good, 10, 250, 4);
        assert_eq!(i, 30);
    }

    #[test]
    fn review_sets_due_and_bumps_reviews() {
        let mut item = InboxItem::capture("id", "b", "cli", "2026-06-01T00:00:00Z");
        item.interval = 10;
        item.ease = 250;
        item.resurface_on = Some("2026-06-10".into());
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let (interval, ease, due, reviews) = review(&item, ReviewResponse::Good, today);
        assert_eq!(ease, 250);
        assert_eq!(interval, 25); // (10 + 0) * 2.5
        assert_eq!(reviews, 1);
        assert_eq!(due, "2026-07-05"); // 2026-06-10 + 25 days
    }
}
