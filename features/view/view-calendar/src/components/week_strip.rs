//! The phone's week strip — seven days under the page title, the way
//! Calendar.app and Linear's iOS app put the week in reach of a thumb.
//!
//! Tap a day to go to it; swipe the strip sideways for the week before
//! or after. Today's number is in the accent colour; the chosen day sits
//! in a filled circle. A dot under a day says it has something planned.

use chrono::{Datelike, Duration, NaiveDate};
use dioxus::prelude::*;

use crate::time::week_days;

/// How far a finger has to travel sideways to page the week.
const SWIPE_PX: f64 = 48.0;

#[component]
pub fn WeekStrip(
    /// The day in view.
    anchor: NaiveDate,
    on_pick: EventHandler<NaiveDate>,
    /// Days with something on them (blocks or events), for the dots.
    #[props(default)]
    busy: Vec<NaiveDate>,
    /// Sized to share a header row with its actions, not to span it.
    #[props(default = false)]
    compact: bool,
) -> Element {
    let today = chrono::Local::now().date_naive();
    let days = week_days(anchor);
    let mut start_x = use_signal(|| None::<f64>);
    rsx! {
        div {
            class: if compact { "grid grid-cols-7 select-none" } else { "grid grid-cols-7 px-2 pb-2 select-none" },
            style: "touch-action: pan-y;",
            onpointerdown: move |e: PointerEvent| start_x.set(Some(e.client_coordinates().x)),
            onpointerup: move |e: PointerEvent| {
                if let Some(x0) = start_x.take() {
                    let dx = e.client_coordinates().x - x0;
                    if dx.abs() > SWIPE_PX {
                        // Finger left → the next week, as a page turns.
                        let weeks = if dx < 0.0 { 7 } else { -7 };
                        on_pick.call(anchor + Duration::days(weeks));
                    }
                }
            },
            onpointercancel: move |_| start_x.set(None),
            for d in days {
                {
                    let picked = d == anchor;
                    let is_today = d == today;
                    let has = busy.contains(&d);
                    let num = if picked {
                        if compact { "flex size-8 items-center justify-center rounded-full bg-primary text-[15px] font-semibold text-primary-foreground" } else { "flex size-9 items-center justify-center rounded-full bg-primary text-[17px] font-semibold text-primary-foreground" }
                    } else if is_today {
                        if compact { "flex size-8 items-center justify-center rounded-full text-[15px] font-semibold text-primary" } else { "flex size-9 items-center justify-center rounded-full text-[17px] font-semibold text-primary" }
                    } else {
                        if compact { "flex size-8 items-center justify-center rounded-full text-[15px] text-foreground" } else { "flex size-9 items-center justify-center rounded-full text-[17px] text-foreground" }
                    };
                    rsx! {
                        button {
                            key: "{d}",
                            r#type: "button",
                            class: if compact { "flex flex-col items-center gap-0.5" } else { "flex flex-col items-center gap-1 py-1" },
                            aria_label: "{d.format(\"%A, %B %-d\")}",
                            aria_pressed: "{picked}",
                            onclick: move |_| on_pick.call(d),
                            span { class: if is_today { "text-[11px] font-semibold uppercase text-primary" } else { "text-[11px] font-medium uppercase text-muted-foreground" },
                                {d.format("%a").to_string().chars().next().map(String::from).unwrap_or_default()}
                            }
                            span { class: num, "{d.day()}" }
                            span { class: if has && !picked { "size-1 rounded-full bg-muted-foreground/60" } else { "size-1" } }
                        }
                    }
                }
            }
        }
    }
}

/// How the page names the day in view: "Today", "Tomorrow",
/// "Yesterday", else "Wednesday" within the week, else "Oct 14".
#[must_use]
pub fn day_title(anchor: NaiveDate, today: NaiveDate) -> String {
    match (anchor - today).num_days() {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        -1 => "Yesterday".to_owned(),
        n if n.abs() < 7 => anchor.format("%A").to_string(),
        _ if anchor.year() == today.year() => anchor.format("%b %-d").to_string(),
        _ => anchor.format("%b %-d, %Y").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_says_the_day_the_way_people_do() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let d = |n| today + Duration::days(n);
        assert_eq!(day_title(today, today), "Today");
        assert_eq!(day_title(d(1), today), "Tomorrow");
        assert_eq!(day_title(d(-1), today), "Yesterday");
        assert_eq!(day_title(d(3), today), "Thursday");
        assert_eq!(day_title(d(30), today), "Nov 11");
        assert_eq!(day_title(d(400), today), "Nov 16, 2027");
    }
}
