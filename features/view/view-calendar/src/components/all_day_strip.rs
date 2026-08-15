//! All-day chip strip above the time grid (week / day views).
//!
//! Tasks-with-only-a-due-date and multi-day events ("Vacation",
//! "Conference") land here instead of stretching the time column.
//! Layout reuses the month view's `track_layout` — same connected-
//! pill rendering with edge squaring when an event extends past
//! the visible window.

use chrono::NaiveDate;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::layout::{ChipPlacement, track_layout};
use crate::store::CalendarMutation;
use crate::time::shift_days;
use crate::types::{CalendarEvent, EventId};

use super::drag::{DT_MIME, DragKind, DragState, use_drag_context};

const TRACK_HEIGHT_PX: i64 = 20;

#[derive(Props, Clone, PartialEq)]
pub struct AllDayStripProps {
    pub days: Vec<NaiveDate>,
    pub events: Vec<CalendarEvent>,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<CalendarMutation>,
    pub on_open_editor: EventHandler<EventId>,
}

#[component]
pub fn AllDayStrip(props: AllDayStripProps) -> Element {
    let days_len = props.days.len() as u8;
    if days_len == 0 {
        return rsx! {};
    }
    let chips = track_layout(props.days[0], days_len, &props.events);
    let track_count: u8 = chips.iter().map(|c| c.track + 1).max().unwrap_or(0);
    // Always reserve at least one track-height so the strip stays
    // visible (the renderer needs somewhere to drop a chip when the
    // user creates the first all-day event).
    let strip_height = i64::from(track_count.max(1)) * TRACK_HEIGHT_PX;

    rsx! {
        div {
            class: "grid border-b border-border/60",
            style: "grid-template-columns: 56px repeat({days_len}, 1fr);",
            // Left rail label
            div {
                class: "text-[10px] text-muted-foreground flex items-center justify-end pr-2",
                "all-day"
            }
            // Days strip — single relative container so chips can
            // span columns via the nested grid.
            div {
                class: "relative col-span-{days_len}",
                style: "grid-column: span {days_len}; height: {strip_height}px;",
                // Background drop cells — one per day, transparent.
                BackgroundCells {
                    days: props.days.clone(),
                    readonly: props.readonly,
                    on_event: props.on_event,
                }
                // Chip overlay (above background, below interactive
                // handles inside individual chips).
                div {
                    class: "absolute inset-0 grid pointer-events-none px-0.5",
                    style: "grid-template-columns: repeat({days_len}, 1fr);",
                    for chip in chips {
                        {
                            let id = chip.event.id;
                            let on_open_editor = props.on_open_editor;
                            rsx! {
                                AllDayChip {
                                    key: "{id}-{chip.start_col}",
                                    placement: chip,
                                    readonly: props.readonly,
                                    on_click: move |()| on_open_editor.call(id),
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
struct BackgroundCellsProps {
    days: Vec<NaiveDate>,
    readonly: bool,
    on_event: EventHandler<CalendarMutation>,
}

/// One transparent target per day so drag-drop can land in the
/// correct column. Uses a nested CSS grid the same width as the
/// chip overlay so coordinates line up.
#[component]
fn BackgroundCells(props: BackgroundCellsProps) -> Element {
    let ctx = use_drag_context();
    let days_len = props.days.len();
    rsx! {
        div {
            class: "absolute inset-0 grid",
            style: "grid-template-columns: repeat({days_len}, 1fr);",
            for date in props.days.iter() {
                {
                    let date = *date;
                    let on_event = props.on_event;
                    rsx! {
                        div {
                            key: "{date}",
                            class: "border-r border-border/20 last:border-r-0",
                            ondragover: move |e: Event<DragData>| {
                                if props.readonly { return; }
                                if ctx.state.peek().is_none() { return; }
                                e.prevent_default();
                            },
                            ondrop: move |e: Event<DragData>| {
                                if props.readonly { return; }
                                e.prevent_default();
                                let dt = e.data().data_transfer();
                                let Ok(id) = dt.get_data(DT_MIME).unwrap_or_default().parse::<Uuid>() else { return };
                                let snapshot = ctx.state.peek().clone();
                                let Some(ds) = snapshot else { return };
                                if ds.event != id { return; }
                                let orig_day = ds.orig_start.date_naive();
                                let delta = (date - orig_day).num_days();
                                let (start, end) = shift_days(ds.orig_start, ds.orig_end, delta);
                                on_event.call(CalendarMutation::Reschedule { id, start, end });
                            },
                            // Double-click = create a new all-day
                            // event on this date.
                            ondoubleclick: move |_| {
                                if props.readonly { return; }
                                let start = crate::time::day_start_utc(date);
                                let end = start + chrono::Duration::days(1);
                                let mut event = CalendarEvent::new("New all-day", start, end);
                                event.all_day = true;
                                on_event.call(CalendarMutation::Create { event });
                            },
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct AllDayChipProps {
    placement: ChipPlacement,
    readonly: bool,
    on_click: EventHandler<()>,
}

#[component]
fn AllDayChip(props: AllDayChipProps) -> Element {
    let ctx = use_drag_context();
    let mut drag = ctx.state;
    let p = props.placement.clone();
    let event = p.event.clone();
    let event_id = event.id;
    let stem = event.color.stem();
    let is_dragging = drag.read().as_ref().is_some_and(|d| d.event == event_id);
    let opacity = if is_dragging { "opacity: 0.4;" } else { "" };

    let rounded_l = if p.continues_left {
        "rounded-l-none"
    } else {
        "rounded-l-sm"
    };
    let rounded_r = if p.continues_right {
        "rounded-r-none"
    } else {
        "rounded-r-sm"
    };
    let bg = format!(
        "bg-{stem}-500/40 text-{stem}-50 border-l-2 border-{stem}-500 hover:bg-{stem}-500/55"
    );
    let style = format!(
        "grid-column: {} / span {}; grid-row: {}; {opacity}",
        p.start_col + 1,
        p.span,
        p.track + 1,
    );

    let on_click = props.on_click;

    rsx! {
        div {
            class: "truncate text-[11px] leading-4 px-1.5 py-0.5 mx-px my-px cursor-pointer select-none pointer-events-auto {bg} {rounded_l} {rounded_r}",
            style: "{style}",
            draggable: !props.readonly,
            "data-cal-drag": "true",
            ondragstart: move |e: Event<DragData>| {
                if props.readonly { return; }
                let dt = e.data().data_transfer();
                let _ = dt.set_data(DT_MIME, &event_id.to_string());
                drag.set(Some(DragState {
                    event: event_id,
                    kind: DragKind::Move,
                    orig_start: event.start,
                    orig_end: event.end,
                    color: event.color,
                    title: event.title.clone(),
                    start_page_x: 0.0,
                    start_page_y: 0.0,
                    committed: true,
                }));
            },
            ondragend: move |_| drag.set(None),
            onclick: move |e: MouseEvent| {
                e.stop_propagation();
                on_click.call(());
            },
            "{event.title}"
        }
    }
}
