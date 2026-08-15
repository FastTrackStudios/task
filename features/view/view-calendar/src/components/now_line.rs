//! Current-time marker for the week/day time grid.
//!
//! A thin red horizontal line at `now` on today's column, plus a
//! small dot at the left edge of that column. Renders only when
//! `today` is in the visible window. A background task ticks a
//! signal every 60 seconds so the line creeps without requiring
//! the user to navigate.

use chrono::{NaiveDate, Timelike};
use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct NowLineProps {
    /// Visible day window — used to figure out *which* column is
    /// today (if any).
    pub days: Vec<NaiveDate>,
    /// Pixels per hour, matching the time grid.
    pub px_per_hour: i64,
}

#[component]
pub fn NowLine(props: NowLineProps) -> Element {
    let mut tick = use_signal(chrono::Local::now);
    use_future(move || async move {
        loop {
            dioxus_sdk_time::sleep(std::time::Duration::from_secs(60)).await;
            tick.set(chrono::Local::now());
        }
    });

    let now = *tick.read();
    let today = now.date_naive();
    let Some(col) = props.days.iter().position(|d| *d == today) else {
        return rsx! {};
    };
    let total_minutes = i64::from(now.hour()) * 60 + i64::from(now.minute());
    let top_px = (total_minutes * props.px_per_hour) / 60;
    let col_one = col + 1;
    let time_label = format_now(now);

    rsx! {
        // Time bubble — pinned to the hour-axis rail (grid column 1)
        // at the same vertical as the line. Reads as a "you are
        // here" pill, matching Google Calendar.
        div {
            class: "pointer-events-none relative",
            style: "grid-column: 1;",
            div {
                class: "absolute right-1 px-1 py-px rounded-sm bg-rose-500 text-white text-[10px] font-semibold leading-none z-30 shadow",
                style: "top: {top_px - 7}px;",
                "{time_label}"
            }
        }
        // Day column — the parent time-grid is a CSS grid with
        // `grid-template-columns: 56px repeat(N, 1fr)`. Our
        // `grid-column` lines up with today's day column.
        div {
            class: "pointer-events-none relative",
            style: "grid-column: {col_one + 1};",
            // Filled dot at left edge.
            div {
                class: "absolute w-2.5 h-2.5 rounded-full bg-rose-500 z-20 ring-2 ring-rose-500/30",
                style: "top: {top_px - 5}px; left: -5px;",
            }
            // 2px line across the day column.
            div {
                class: "absolute left-0 right-0 h-0.5 bg-rose-500 z-20",
                style: "top: {top_px - 1}px;",
            }
        }
    }
}

fn format_now(now: chrono::DateTime<chrono::Local>) -> String {
    let h = now.hour();
    let m = now.minute();
    let (h12, suf) = if h == 0 {
        (12, "AM")
    } else if h < 12 {
        (h, "AM")
    } else if h == 12 {
        (12, "PM")
    } else {
        (h - 12, "PM")
    };
    format!("{h12}:{m:02} {suf}")
}
