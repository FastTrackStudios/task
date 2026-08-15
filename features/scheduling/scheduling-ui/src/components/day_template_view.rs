//! Read-only viewer for a personal day template. The feature
//! brief's reference table renders as one of these.
//!
//! Edit affordances (drag time-block edges, inline rename, change
//! category) ship in a follow-up commit; this v1 surface establishes
//! the visual + the prop shape the editor will inherit.

use dioxus::prelude::*;
use architect_ui::prelude::*;

use scheduling_proto::{BlockCategory, DayTemplate, TimeBlock, TimeOfDay};

#[derive(Props, Clone, PartialEq)]
pub struct DayTemplateViewProps {
    pub template: DayTemplate,
}

#[component]
pub fn DayTemplateView(props: DayTemplateViewProps) -> Element {
    let blocks = props.template.blocks.clone();
    rsx! {
        div { class: "flex flex-col gap-3 w-full max-w-3xl",
            div { class: "flex items-baseline gap-3",
                Heading { level: HeadingLevel::H2, "{props.template.name}" }
                if let Some(d) = props.template.description.as_ref() {
                    Text { variant: TextVariant::Muted, class: "text-sm", "{d}" }
                }
            }
            div { class: "rounded-lg border border-border/60 overflow-hidden",
                table { class: "w-full text-sm",
                    thead { class: "bg-card/60",
                        tr {
                            th { class: "text-left px-3 py-2 text-xs uppercase tracking-wide text-muted-foreground w-44", "Time" }
                            th { class: "text-left px-3 py-2 text-xs uppercase tracking-wide text-muted-foreground", "Activity" }
                            th { class: "text-left px-3 py-2 text-xs uppercase tracking-wide text-muted-foreground w-28", "Category" }
                        }
                    }
                    tbody {
                        for b in blocks.iter() {
                            BlockRow { key: "{b.id.0}", block: b.clone() }
                        }
                    }
                }
            }
            Summary { template: props.template }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct BlockRowProps {
    block: TimeBlock,
}

#[component]
fn BlockRow(props: BlockRowProps) -> Element {
    let b = &props.block;
    let stem = category_stem(b.category);
    let chip = format!(
        "inline-flex items-center px-2 py-0.5 rounded-full text-[10px] bg-{stem}-500/20 text-{stem}-100 border border-{stem}-500/30"
    );
    let label = category_label(b.category);
    rsx! {
        tr { class: "border-t border-border/30 hover:bg-card/40",
            td { class: "px-3 py-2 font-mono text-xs tabular-nums",
                "{format_time(b.start)}–{format_time(b.end)}"
            }
            td { class: "px-3 py-2",
                div { class: "flex flex-col",
                    span { class: "text-foreground", "{b.label}" }
                    if let Some(n) = b.note.as_ref() {
                        span { class: "text-xs text-muted-foreground", "{n}" }
                    }
                }
            }
            td { class: "px-3 py-2",
                span { class: "{chip}", "{label}" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct SummaryProps {
    template: DayTemplate,
}

#[component]
fn Summary(props: SummaryProps) -> Element {
    let totals = totals(&props.template);
    rsx! {
        div { class: "flex flex-wrap gap-2",
            for (label, hours) in totals {
                {
                    let key = label;
                    rsx! {
                        div {
                            key: "{key}",
                            class: "px-3 py-1.5 rounded-md border border-border/60 bg-card/40 text-xs flex items-center gap-2",
                            span { class: "text-muted-foreground", "{label}" }
                            span { class: "font-medium tabular-nums", "{hours}" }
                        }
                    }
                }
            }
        }
    }
}

fn format_time(t: TimeOfDay) -> String {
    let h = t.hours();
    let m = t.minutes();
    let (h12, suf) = if h == 0 {
        (12, "AM")
    } else if h < 12 {
        (h, "AM")
    } else if h == 12 {
        (12, "PM")
    } else {
        (h - 12, "PM")
    };
    if m == 0 {
        format!("{h12}{suf}")
    } else {
        format!("{h12}:{m:02}{suf}")
    }
}

fn category_label(c: BlockCategory) -> &'static str {
    match c {
        BlockCategory::Reset => "Reset",
        BlockCategory::Spiritual => "Spiritual",
        BlockCategory::Meal => "Meal",
        BlockCategory::Exercise => "Exercise",
        BlockCategory::Hygiene => "Hygiene",
        BlockCategory::Allocatable => "Block",
        BlockCategory::Maintenance => "Maintenance",
        BlockCategory::WindDown => "Wind down",
        BlockCategory::Sleep => "Sleep",
        BlockCategory::Other => "Other",
    }
}

fn category_stem(c: BlockCategory) -> &'static str {
    match c {
        BlockCategory::Reset => "sky",
        BlockCategory::Spiritual => "violet",
        BlockCategory::Meal => "amber",
        BlockCategory::Exercise => "rose",
        BlockCategory::Hygiene => "cyan",
        BlockCategory::Allocatable => "emerald",
        BlockCategory::Maintenance => "yellow",
        BlockCategory::WindDown => "indigo",
        BlockCategory::Sleep => "slate",
        BlockCategory::Other => "neutral",
    }
}

/// Sum durations per category, returning ordered (label, "Hh Mm")
/// pairs for the summary chips. Lets the user check at a glance
/// that the brief's daily totals match.
fn totals(dt: &DayTemplate) -> Vec<(&'static str, String)> {
    let mut by_cat: Vec<(BlockCategory, i32)> = Vec::new();
    for b in dt.blocks.iter() {
        let mins =
            i32::from(b.end.minutes_since_midnight) - i32::from(b.start.minutes_since_midnight);
        if let Some(entry) = by_cat
            .iter_mut()
            .find(|(c, _)| *c as u8 == b.category as u8)
        {
            entry.1 += mins;
        } else {
            by_cat.push((b.category, mins));
        }
    }
    by_cat
        .into_iter()
        .map(|(c, mins)| {
            (
                category_label(c),
                format!("{}h {:02}m", mins / 60, mins % 60),
            )
        })
        .collect()
}
