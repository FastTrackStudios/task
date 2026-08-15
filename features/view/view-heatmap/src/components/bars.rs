//! Weekly-bar view inspired by `CodexMonitor`'s usage chart: one
//! vertical bar per day of the visible week, height ∝ count, with
//! prev/next chevrons to walk through history.

use std::collections::HashMap;

use chrono::{Datelike, Days, NaiveDate, Weekday};
use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{ChevronLeft, ChevronRight};

use crate::intensity::ColorTag;

#[derive(Props, Clone, PartialEq)]
pub struct BarsViewProps {
    /// Anchor date — the visible week is the week containing this
    /// date. Internal week-offset signal navigates backward.
    pub anchor: NaiveDate,
    pub points: Vec<(NaiveDate, u32)>,
    pub color: ColorTag,
}

#[component]
pub fn BarsView(props: BarsViewProps) -> Element {
    let mut offset: Signal<i64> = use_signal(|| 0);
    let base_monday = monday_of(props.anchor);
    let monday = base_monday - Days::new((*offset.read() * 7) as u64);
    let days: Vec<NaiveDate> = (0..7).map(|i| monday + Days::new(i)).collect();

    let mut counts: HashMap<NaiveDate, u32> = HashMap::new();
    for (d, n) in &props.points {
        *counts.entry(*d).or_insert(0) += n;
    }
    let max_in_week = days
        .iter()
        .map(|d| counts.get(d).copied().unwrap_or(0))
        .max()
        .unwrap_or(0)
        .max(1);

    let stem = props.color.stem();
    let range_label = format!(
        "{} – {}",
        monday.format("%b %-d"),
        (monday + Days::new(6)).format("%b %-d, %Y"),
    );

    rsx! {
        div { class: "flex flex-col gap-2",
            // Nav
            div { class: "flex items-center gap-2",
                button {
                    r#type: "button",
                    class: "p-1 rounded hover:bg-accent",
                    onclick: move |_| offset.with_mut(|o| *o += 1),
                    ChevronLeft { size: 14 }
                }
                span { class: "text-xs text-muted-foreground", "{range_label}" }
                button {
                    r#type: "button",
                    class: "p-1 rounded hover:bg-accent disabled:opacity-40",
                    disabled: *offset.read() <= 0,
                    onclick: move |_| offset.with_mut(|o| if *o > 0 { *o -= 1 }),
                    ChevronRight { size: 14 }
                }
            }
            // Bars — 7 columns, each a flex-end column so bars
            // grow from the baseline.
            div { class: "flex items-end gap-1 h-32",
                for date in days.iter() {
                    {
                        let count = counts.get(date).copied().unwrap_or(0);
                        let pct = ((count as f32 / max_in_week as f32) * 100.0).max(2.0);
                        let day_label = date.format("%a").to_string();
                        let day_num = date.day();
                        let tooltip = format!("{} · {} {}", date.format("%a %b %-d"), count, if count == 1 { "entry" } else { "entries" });
                        rsx! {
                            div {
                                key: "{date}",
                                class: "flex flex-col items-center gap-1 flex-1 group",
                                title: "{tooltip}",
                                div { class: "w-full h-full flex flex-col justify-end",
                                    div {
                                        class: "w-full rounded-t-sm bg-{stem}-500/70 group-hover:bg-{stem}-400 transition-colors",
                                        style: "height: {pct}%;",
                                    }
                                }
                                div { class: "text-[10px] text-muted-foreground", "{day_label} {day_num}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn monday_of(d: NaiveDate) -> NaiveDate {
    let offset = match d.weekday() {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    d - Days::new(offset)
}
