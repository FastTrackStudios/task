//! GitHub-style contribution grid: 53 columns (weeks) × 7 rows
//! (days). The trailing column is the week containing `anchor`.
//!
//! Layout is column-major (one `<div>` per week, seven cells per
//! column) so CSS grid flows naturally and weekday rows stay
//! aligned.

use std::collections::HashMap;

use chrono::{Datelike, Days, NaiveDate, Weekday};
use dioxus::prelude::*;

use crate::intensity::{ColorTag, IntensityBucket, bucket_for};

/// Number of weeks shown. 53 covers a calendar year + the
/// partial-week tail that GitHub also paints.
const WEEKS: i64 = 53;

#[derive(Props, Clone, PartialEq)]
pub struct GridViewProps {
    pub anchor: NaiveDate,
    pub points: Vec<(NaiveDate, u32)>,
    pub color: ColorTag,
}

#[component]
pub fn GridView(props: GridViewProps) -> Element {
    // Anchor at Saturday of `anchor`'s week so the trailing column
    // is complete-looking (GitHub uses Sunday → Saturday weeks but
    // we keep the Monday-start convention used by view-calendar to
    // match the rest of the app).
    let monday = monday_of(props.anchor);
    let start_monday = monday - Days::new(((WEEKS - 1) * 7) as u64);

    // Sum counts per day.
    let mut counts: HashMap<NaiveDate, u32> = HashMap::new();
    for (d, n) in &props.points {
        *counts.entry(*d).or_insert(0) += n;
    }
    let max = counts.values().copied().max().unwrap_or(0);
    let stem = props.color.stem();

    // Month labels along the top — show the abbreviation when the
    // first cell of a column lands in a new month.
    let mut last_month: Option<u32> = None;

    rsx! {
        div { class: "flex flex-col gap-1",
            // Month label strip
            div { class: "flex gap-[3px] pl-8 text-[10px] text-muted-foreground",
                for w in 0..WEEKS {
                    {
                        let col_start = start_monday + Days::new((w * 7) as u64);
                        let label = if last_month != Some(col_start.month()) && col_start.day() <= 7 {
                            last_month = Some(col_start.month());
                            col_start.format("%b").to_string()
                        } else {
                            String::new()
                        };
                        rsx! {
                            div { key: "m-{w}", class: "w-[11px] text-center", "{label}" }
                        }
                    }
                }
            }
            // Body — left rail of weekday labels + 53 week columns
            div { class: "flex gap-1",
                // Weekday rail (Mon, Wed, Fri visible, others blank)
                div { class: "flex flex-col gap-[3px] text-[10px] text-muted-foreground w-8 shrink-0",
                    for dow_idx in 0..7u8 {
                        {
                            let label = match dow_idx {
                                0 => "Mon",
                                2 => "Wed",
                                4 => "Fri",
                                _ => "",
                            };
                            rsx! {
                                div { key: "dow-{dow_idx}", class: "h-[11px] leading-none", "{label}" }
                            }
                        }
                    }
                }
                // Week columns
                div { class: "flex gap-[3px]",
                    for w in 0..WEEKS {
                        {
                            let col_start = start_monday + Days::new((w * 7) as u64);
                            rsx! {
                                div { key: "w-{w}", class: "flex flex-col gap-[3px]",
                                    for dow in 0..7u8 {
                                        {
                                            let date = col_start + Days::new(u64::from(dow));
                                            let count = counts.get(&date).copied().unwrap_or(0);
                                            let bucket = bucket_for(count, max);
                                            let cell_class = cell_class(stem, bucket);
                                            let tooltip = format!(
                                                "{} · {} {}",
                                                date.format("%a %b %-d, %Y"),
                                                count,
                                                if count == 1 { "entry" } else { "entries" },
                                            );
                                            rsx! {
                                                div {
                                                    key: "{date}",
                                                    class: "w-[11px] h-[11px] rounded-sm {cell_class}",
                                                    title: "{tooltip}",
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Legend
            div { class: "flex items-center gap-2 text-[10px] text-muted-foreground self-end pr-1",
                "Less"
                for bucket in [
                    IntensityBucket::Zero,
                    IntensityBucket::One,
                    IntensityBucket::Two,
                    IntensityBucket::Three,
                    IntensityBucket::Four,
                ] {
                    {
                        let cls = cell_class(stem, bucket);
                        rsx! { div { key: "leg-{bucket:?}", class: "w-[11px] h-[11px] rounded-sm {cls}" } }
                    }
                }
                "More"
            }
        }
    }
}

fn cell_class(stem: &str, bucket: IntensityBucket) -> String {
    match bucket {
        IntensityBucket::Zero => "bg-muted/40".to_string(),
        IntensityBucket::One => format!("bg-{stem}-500/20"),
        IntensityBucket::Two => format!("bg-{stem}-500/40"),
        IntensityBucket::Three => format!("bg-{stem}-500/65"),
        IntensityBucket::Four => format!("bg-{stem}-500/90"),
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
