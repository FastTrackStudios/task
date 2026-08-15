//! Cyclic-planning heatmap.
//!
//! Renders the year as **4 quarters × 13 weeks**, with each quarter
//! visibly grouped into 3 cycles of 4 weeks + 1 reset week. Cyclic
//! leap years tack a 53rd "bonus week / week 0" onto the end. See
//! [`crate::cyclic`] for the date math.
//!
//! Layout:
//!
//! ```text
//! Q1 │ C1: ▢▢▢▢  C2: ▢▢▢▢  C3: ▢▢▢▢  R: ▢
//!     │ (7 day rows tall for each ▢)
//! Q2 │ …
//! Q3 │ …
//! Q4 │ …  ‖  W0 (bonus, only on leap years)
//! ```
//!
//! Cycle separators show as thin gaps. Reset week + bonus week get
//! a subtle ring so the user can tell at a glance "this is a reset
//! week, treat it differently from a regular work week."

use std::collections::HashMap;

use chrono::{Datelike, Days, NaiveDate, Weekday};
use dioxus::prelude::*;

use crate::cyclic::{CyclicConfig, WeekSlot};
use crate::intensity::{ColorTag, IntensityBucket, bucket_for};

#[derive(Props, Clone, PartialEq)]
pub struct CyclicViewProps {
    /// Anchor date — its calendar year picks the cyclic year.
    pub anchor: NaiveDate,
    pub points: Vec<(NaiveDate, u32)>,
    pub color: ColorTag,
    /// Day of week to start each cyclic week on. Defaults to
    /// Monday (the convention used in the source video).
    #[props(default)]
    pub week_start: Option<Weekday>,
}

#[component]
pub fn CyclicView(props: CyclicViewProps) -> Element {
    let week_start = props.week_start.unwrap_or(Weekday::Mon);
    let config = CyclicConfig::for_year(props.anchor.year(), week_start);
    let stem = props.color.stem();

    // Sum counts per day; max for bucketing.
    let mut counts: HashMap<NaiveDate, u32> = HashMap::new();
    for (d, n) in &props.points {
        *counts.entry(*d).or_insert(0) += n;
    }
    let max = counts.values().copied().max().unwrap_or(0);

    // Walk weeks and bucket by quarter so we can render 4 quarter
    // grids in document order.
    let weeks: Vec<_> = config.weeks().collect();
    let quarters: Vec<Vec<_>> = (0..4u8)
        .map(|q| weeks.iter().filter(|w| w.quarter == q).copied().collect())
        .collect();
    let bonus: Option<_> = weeks.iter().find(|w| w.quarter == 4).copied();

    let weekday_labels = weekday_labels(week_start);

    rsx! {
        div { class: "flex flex-col gap-4",
            // Year + leap-year badge
            div { class: "flex items-baseline gap-3",
                span { class: "text-lg font-semibold tracking-tight", "Cyclic {config.year}" }
                span { class: "text-xs text-muted-foreground",
                    "Q1 · Q2 · Q3 · Q4   ·   3 cycles + reset week each"
                }
                if config.bonus_week {
                    span {
                        class: "px-2 py-0.5 rounded-full text-[10px] bg-{stem}-500/20 text-{stem}-100 border border-{stem}-500/30",
                        "Cyclic leap year · +1 bonus week"
                    }
                }
            }
            // Four quarters
            div { class: "flex flex-col gap-3",
                for (q_idx, q_weeks) in quarters.iter().enumerate() {
                    {
                        let q_weeks = q_weeks.clone();
                        let labels = weekday_labels.clone();
                        let counts = counts.clone();
                        rsx! {
                            QuarterRow {
                                key: "q{q_idx}",
                                title: format!("Q{}", q_idx + 1),
                                range_label: range_label(&q_weeks),
                                weeks: q_weeks,
                                weekday_labels: labels,
                                stem: stem.to_string(),
                                counts,
                                max,
                            }
                        }
                    }
                }
                // Bonus week, if any.
                if let Some(bonus) = bonus {
                    {
                        let labels = weekday_labels.clone();
                        let stem = stem.to_string();
                        let counts = counts.clone();
                        let bonus_key = format!("q-bonus-{}", bonus.start);
                        rsx! {
                            QuarterRow {
                                key: "{bonus_key}",
                                title: "W0".to_string(),
                                range_label: format!(
                                    "{} – {}",
                                    bonus.start.format("%b %-d"),
                                    (bonus.start + Days::new(6)).format("%b %-d"),
                                ),
                                weeks: vec![bonus],
                                weekday_labels: labels,
                                stem,
                                counts,
                                max,
                            }
                        }
                    }
                }
            }
            // Legend
            Legend { stem: stem.to_string() }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct QuarterRowProps {
    title: String,
    range_label: String,
    weeks: Vec<crate::cyclic::WeekCoord>,
    weekday_labels: Vec<&'static str>,
    stem: String,
    counts: HashMap<NaiveDate, u32>,
    max: u32,
}

#[component]
fn QuarterRow(props: QuarterRowProps) -> Element {
    rsx! {
        div { class: "flex items-start gap-3",
            // Left rail: quarter title + date range
            div { class: "w-16 shrink-0 flex flex-col pt-3",
                span { class: "text-sm font-semibold", "{props.title}" }
                span { class: "text-[10px] text-muted-foreground leading-tight", "{props.range_label}" }
            }
            // Weekday row labels + week columns
            div { class: "flex gap-1",
                // Weekday rail
                div { class: "flex flex-col gap-[3px] pt-3 text-[10px] text-muted-foreground w-7 shrink-0",
                    for (i, l) in props.weekday_labels.iter().enumerate() {
                        div { key: "wl-{i}", class: "h-[14px] leading-none", "{l}" }
                    }
                }
                // Cycles + reset week. Group with a small gap
                // between cycles to make the 4+4+4+1 structure
                // visually obvious.
                CycleGroups {
                    weeks: props.weeks,
                    stem: props.stem,
                    counts: props.counts,
                    max: props.max,
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct CycleGroupsProps {
    weeks: Vec<crate::cyclic::WeekCoord>,
    stem: String,
    counts: HashMap<NaiveDate, u32>,
    max: u32,
}

#[component]
fn CycleGroups(props: CycleGroupsProps) -> Element {
    // Group consecutive weeks by their cycle (or Reset / Bonus).
    // Outputs a Vec<(label, weeks)> so each group can be rendered
    // with its own header + spacing.
    let groups = group_weeks(&props.weeks);

    rsx! {
        div { class: "flex items-start gap-2",
            for (g_idx, (label, group)) in groups.into_iter().enumerate() {
                {
                    let stem = props.stem.clone();
                    let counts = props.counts.clone();
                    let max = props.max;
                    let group_class = match label {
                        GroupLabel::Cycle(_) => "rounded-md p-1 border border-transparent",
                        GroupLabel::Reset => "rounded-md p-1 border border-{stem}-500/40 bg-{stem}-500/5",
                        GroupLabel::Bonus => "rounded-md p-1 border border-dashed border-{stem}-500/60",
                    };
                    let header = match label {
                        GroupLabel::Cycle(n) => format!("C{}", n + 1),
                        GroupLabel::Reset => "Reset".to_string(),
                        GroupLabel::Bonus => "W0".to_string(),
                    };
                    rsx! {
                        div {
                            key: "g{g_idx}",
                            class: "flex flex-col gap-1 {group_class}",
                            div {
                                class: "text-[10px] tracking-wide text-muted-foreground px-0.5",
                                "{header}"
                            }
                            div { class: "flex gap-[3px]",
                                for w in group.iter() {
                                    {
                                        let w = *w;
                                        let stem = stem.clone();
                                        let counts = counts.clone();
                                        rsx! { WeekColumn { key: "w-{w.start}", week: w, stem, counts, max } }
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

#[derive(Props, Clone, PartialEq)]
struct WeekColumnProps {
    week: crate::cyclic::WeekCoord,
    stem: String,
    counts: HashMap<NaiveDate, u32>,
    max: u32,
}

#[component]
fn WeekColumn(props: WeekColumnProps) -> Element {
    let start = props.week.start;
    let stem = props.stem.as_str();
    rsx! {
        div { class: "flex flex-col gap-[3px]",
            for dow in 0..7u8 {
                {
                    let date = start + Days::new(u64::from(dow));
                    let count = props.counts.get(&date).copied().unwrap_or(0);
                    let bucket = bucket_for(count, props.max);
                    let cls = cell_class(stem, bucket);
                    let tooltip = format!(
                        "{} · {} {}",
                        date.format("%a %b %-d, %Y"),
                        count,
                        if count == 1 { "entry" } else { "entries" },
                    );
                    rsx! {
                        div {
                            key: "{date}",
                            class: "w-[14px] h-[14px] rounded-sm {cls}",
                            title: "{tooltip}",
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct LegendProps {
    stem: String,
}

#[component]
fn Legend(props: LegendProps) -> Element {
    let stem = props.stem.as_str();
    rsx! {
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
                    rsx! { div { key: "leg-{bucket:?}", class: "w-[12px] h-[12px] rounded-sm {cls}" } }
                }
            }
            "More"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupLabel {
    Cycle(u8),
    Reset,
    Bonus,
}

fn group_weeks(
    weeks: &[crate::cyclic::WeekCoord],
) -> Vec<(GroupLabel, Vec<crate::cyclic::WeekCoord>)> {
    let mut out: Vec<(GroupLabel, Vec<crate::cyclic::WeekCoord>)> = Vec::new();
    let mut current: Option<(GroupLabel, Vec<crate::cyclic::WeekCoord>)> = None;
    for w in weeks {
        let label = match w.slot {
            WeekSlot::Cycle { cycle, .. } => GroupLabel::Cycle(cycle),
            WeekSlot::Reset => GroupLabel::Reset,
            WeekSlot::Bonus => GroupLabel::Bonus,
        };
        match &mut current {
            Some((lbl, group)) if *lbl == label => group.push(*w),
            _ => {
                if let Some(prev) = current.take() {
                    out.push(prev);
                }
                current = Some((label, vec![*w]));
            }
        }
    }
    if let Some(prev) = current {
        out.push(prev);
    }
    out
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

fn weekday_labels(week_start: Weekday) -> Vec<&'static str> {
    let labels: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    // Rotate so the start day appears first.
    let offset = match week_start {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    (0..7).map(|i| labels[(i + offset) % 7]).collect()
}

fn range_label(weeks: &[crate::cyclic::WeekCoord]) -> String {
    let Some(first) = weeks.first() else {
        return String::new();
    };
    let Some(last) = weeks.last() else {
        return String::new();
    };
    let end = last.start + Days::new(6);
    format!(
        "{} – {}",
        first.start.format("%b %-d"),
        end.format("%b %-d")
    )
}
