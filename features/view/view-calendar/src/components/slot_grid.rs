//! A week grid whose rows are *categories*, not hours.
//!
//! The rest of this crate maps a day onto a clock: a block's geometry
//! is `start_min * px_per_hour / 60`, the axis is hours, and dragging
//! converts pointer-y back into snapped minutes. That is the right
//! model for something that happens *at* a time.
//!
//! Plenty of planning isn't like that. A meal is "Friday dinner", not
//! "Friday 18:30–19:15"; a habit is "Tuesday morning"; a shift is
//! "Saturday, late". Forcing those onto the time grid means inventing
//! clock values nobody chose, and then living with the consequences —
//! blocks that drift when dragged, a now-line implying precision the
//! data doesn't have.
//!
//! So this is the categorical sibling: caller-supplied rows crossed
//! with the seven days of a week, cells holding chips. It shares the
//! crate's palette, date maths and vocabulary, so a slot grid and a
//! time grid look like the same product — they just disagree about
//! what the vertical axis means.
//!
//! It is deliberately generic. Nothing here knows about meals; the
//! caller names its own rows and maps its own items.

use chrono::{Datelike, NaiveDate};
use dioxus::prelude::*;

use crate::components::style::chip_palette;
use crate::types::ColorTag;

/// One row of the grid — a category, in the order the caller wants it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRow {
    /// Matched against [`SlotItem::slot`].
    pub key: String,
    /// What the rail shows.
    pub label: String,
}

impl SlotRow {
    #[must_use]
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

/// Something that sits in one (day, slot) cell.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotItem {
    /// Stable identity — used as the render key and passed back on click.
    pub id: String,
    pub date: NaiveDate,
    /// Matched against [`SlotRow::key`].
    pub slot: String,
    pub label: String,
    /// Optional second line: a serving count, a time, a note.
    pub detail: Option<String>,
    pub color: ColorTag,
    /// Drawn as struck through and dimmed — cancelled, skipped, eaten
    /// elsewhere. Still shown, because "we deliberately didn't" is
    /// information the week should carry.
    pub muted: bool,
}

impl SlotItem {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        date: NaiveDate,
        slot: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            date,
            slot: slot.into(),
            label: label.into(),
            detail: None,
            color: ColorTag::Primary,
            muted: false,
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn color(mut self, color: ColorTag) -> Self {
        self.color = color;
        self
    }

    #[must_use]
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SlotGridProps {
    /// The columns. Usually [`crate::time::week_days`], but any run of
    /// dates works — three days on a narrow screen, a fortnight on a
    /// wide one.
    pub days: Vec<NaiveDate>,
    /// The rows, in display order.
    pub rows: Vec<SlotRow>,
    pub items: Vec<SlotItem>,
    /// Highlighted column. Pass `None` when the grid isn't showing the
    /// current week, so nothing is falsely marked as today.
    #[props(default)]
    pub today: Option<NaiveDate>,
    /// An item was clicked, by [`SlotItem::id`].
    #[props(default)]
    pub on_item: Option<EventHandler<String>>,
    /// An empty part of a cell was clicked — `(date, slot key)`. This is
    /// how a caller offers "plan something here" without this component
    /// needing to know what that means.
    #[props(default)]
    pub on_cell: Option<EventHandler<(NaiveDate, String)>>,
    /// Minimum width before the grid scrolls sideways. Seven columns
    /// don't fit a phone, and squeezing them to fit destroys the one
    /// thing the view is for.
    ///
    /// Sized so a full week clears a desktop content column *with* a
    /// sidebar open: the whole point is seeing the week at once, and a
    /// grid that pushes Sunday behind a scrollbar on a 1440px screen
    /// has failed at its only job.
    #[props(default = "39rem".to_string())]
    pub min_width: String,
}

#[component]
pub fn SlotGrid(props: SlotGridProps) -> Element {
    let SlotGridProps {
        days,
        rows,
        items,
        today,
        on_item,
        on_cell,
        min_width,
    } = props;

    let cols = days.len().max(1);
    let template = format!("grid-template-columns: 4.5rem repeat({cols}, minmax(4.75rem, 1fr));");

    rsx! {
        div { class: "overflow-x-auto rounded-2xl border border-border bg-card/30",
            div {
                class: "grid",
                style: "min-width: {min_width}; {template}",

                // ── Day header ──────────────────────────────────
                div { class: "sticky left-0 z-10 border-b border-border bg-card/80 backdrop-blur" }
                for d in days.iter().copied() {
                    {
                        let is_today = today == Some(d);
                        let cell = if is_today {
                            "border-b border-border bg-primary/10 px-2 py-2 text-center"
                        } else {
                            "border-b border-border px-2 py-2 text-center"
                        };
                        rsx! {
                            div { key: "h-{d}", class: "{cell}",
                                div { class: "text-[11px] uppercase tracking-wide text-muted-foreground",
                                    "{d.format(\"%a\")}"
                                }
                                div {
                                    class: if is_today {
                                        "text-sm font-semibold tabular-nums text-foreground"
                                    } else {
                                        "text-sm tabular-nums text-muted-foreground"
                                    },
                                    "{d.day()}"
                                }
                            }
                        }
                    }
                }

                // ── One row per slot ────────────────────────────
                for (ri, row) in rows.iter().enumerate() {
                    {
                        let last = ri + 1 == rows.len();
                        let edge = if last { "" } else { "border-b border-border/50" };
                        rsx! {
                            div {
                                key: "r-{row.key}",
                                class: "sticky left-0 z-10 flex items-start bg-card/80 px-3 py-2.5 backdrop-blur {edge}",
                                span { class: "text-xs font-semibold uppercase tracking-[0.12em] text-muted-foreground",
                                    "{row.label}"
                                }
                            }
                            for d in days.iter().copied() {
                                {
                                    let here: Vec<&SlotItem> = items
                                        .iter()
                                        .filter(|i| i.date == d && i.slot == row.key)
                                        .collect();
                                    let is_today = today == Some(d);
                                    let tint = if is_today { "bg-primary/5" } else { "" };
                                    let key = row.key.clone();
                                    rsx! {
                                        div {
                                            key: "c-{d}-{row.key}",
                                            class: "flex min-h-[3rem] flex-col gap-1 border-l border-border/40 px-1.5 py-1.5 {edge} {tint}",
                                            onclick: move |_| {
                                                if let Some(h) = &on_cell {
                                                    h.call((d, key.clone()));
                                                }
                                            },
                                            for item in here {
                                                {
                                                    let pal = chip_palette(item.color);
                                                    let dim = if item.muted {
                                                        "opacity-60 line-through"
                                                    } else {
                                                        ""
                                                    };
                                                    let id = item.id.clone();
                                                    rsx! {
                                                        button {
                                                            key: "{item.id}",
                                                            class: "w-full rounded-md px-1.5 py-1 text-left text-[11px] leading-snug transition-colors {pal.body} {pal.hover} {dim}",
                                                            title: "{item.label}",
                                                            onclick: move |e| {
                                                                // The cell behind this also listens;
                                                                // a click on a chip means the chip.
                                                                e.stop_propagation();
                                                                if let Some(h) = &on_item {
                                                                    h.call(id.clone());
                                                                }
                                                            },
                                                            // Wrapped to two lines rather than truncated:
                                                            // "Monday O…" and "Monday —…" are the same
                                                            // chip as far as a reader is concerned, and a
                                                            // week you can't read is not a week view.
                                                            div { class: "line-clamp-2 font-medium", "{item.label}" }
                                                            if let Some(detail) = &item.detail {
                                                                div { class: "truncate opacity-80", "{detail}" }
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
                    }
                }
            }
        }
    }
}
