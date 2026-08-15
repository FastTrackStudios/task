//! Month view — 6 week rows, each laid out as a 7-column grid.
//!
//! Multi-day events render as a single connected chip spanning day
//! columns inside the row (Google Calendar style). Chips that
//! extend past either week edge square off the cut corner via
//! `continues_left` / `continues_right` from the layout pass.
//!
//! Each week row stacks:
//! 1. **Day-header strip**: the seven date numbers.
//! 2. **Chip rows**: zero or more "tracks" of event chips. Each
//!    track is a CSS grid the same width as the day header so the
//!    chips align perfectly under the dates.
//! 3. **Click target**: a single absolutely-positioned layer behind
//!    the chips that catches clicks → create event, and drops →
//!    reschedule.

use chrono::{Datelike, NaiveDate};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::layout::{MonthChipPlacement, month_week_layout};
use crate::store::CalendarMutation;
use crate::time::{day_start_utc, month_grid, shift_days};
use crate::types::{CalendarEvent, EventId};

use super::drag::{DT_MIME, use_drag_context, use_touch_context};
use super::style::chip_palette;

/// Cap of visible chip tracks per week row before the renderer
/// folds overflow into a `+N more` link at the bottom of each
/// affected cell. Matches Google Calendar's default of three rows
/// per cell.
const MAX_VISIBLE_TRACKS: u8 = 3;

#[derive(Props, Clone, PartialEq)]
pub struct MonthViewProps {
    /// Any date inside the month to render.
    pub anchor: NaiveDate,
    pub events: Vec<CalendarEvent>,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<CalendarMutation>,
    pub on_open_editor: EventHandler<EventId>,
    /// Clicked the "+N more" link in a day cell — Calendar root
    /// switches to Day view anchored at that date (matches Google).
    pub on_zoom_to_day: EventHandler<NaiveDate>,
}

#[component]
pub fn MonthView(props: MonthViewProps) -> Element {
    let grid = month_grid(props.anchor);
    let cur_month = props.anchor.month();
    let today = chrono::Local::now().date_naive();

    let weekday_labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

    rsx! {
        div { class: "flex flex-col h-full w-full",
            // Weekday header strip
            div { class: "grid grid-cols-7 border-b border-border/60 text-xs text-muted-foreground",
                for label in weekday_labels {
                    div { key: "{label}", class: "px-2 py-1 text-center font-medium", "{label}" }
                }
            }
            // 6 week rows. Scrolls when the rows' min-height exceeds
            // the container (phone-height viewports — the cells keep
            // their ≥6rem touch size instead of squashing).
            div { class: "flex-1 min-h-0 grid grid-rows-6 overflow-y-auto",
                for row in 0..6u8 {
                    {
                        let row_days = grid[row as usize];
                        let row_start = row_days[0];
                        let chips = month_week_layout(row_start, &props.events);
                        rsx! {
                            WeekRow {
                                key: "{row_start}",
                                days: row_days,
                                cur_month,
                                today,
                                chips,
                                is_last_row: row == 5,
                                readonly: props.readonly,
                                on_event: props.on_event,
                                on_open_editor: props.on_open_editor,
                                on_zoom_to_day: props.on_zoom_to_day,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct WeekRowProps {
    days: [NaiveDate; 7],
    cur_month: u32,
    today: NaiveDate,
    chips: Vec<MonthChipPlacement>,
    is_last_row: bool,
    readonly: bool,
    on_event: EventHandler<CalendarMutation>,
    on_open_editor: EventHandler<EventId>,
    on_zoom_to_day: EventHandler<NaiveDate>,
}

#[component]
fn WeekRow(props: WeekRowProps) -> Element {
    let border_b = if props.is_last_row { "" } else { "border-b" };
    // Touch-first behavior: on a coarse pointer the whole day cell is
    // the tap target (≥44px both ways) and a tap *navigates to that
    // day's Day view* instead of creating an event — the Google
    // mobile pattern. Chips are too small for finger-accurate taps in
    // a month cell, so they fall through to the cell (see MonthChip).
    let coarse = *use_touch_context().coarse.read();

    // Split chips into visible vs. overflow tracks. Overflow chips
    // never paint; they only bump a per-cell counter so we can
    // render a "+N more" link in each column they would have
    // touched.
    let (visible_chips, hidden_chips): (Vec<_>, Vec<_>) = props
        .chips
        .iter()
        .cloned()
        .partition(|c| c.track < MAX_VISIBLE_TRACKS);
    let mut overflow_per_col: [u8; 7] = [0; 7];
    for c in &hidden_chips {
        let end = (c.start_col + c.span).min(7);
        for col in c.start_col..end {
            overflow_per_col[col as usize] = overflow_per_col[col as usize].saturating_add(1);
        }
    }
    let tracks: u8 = visible_chips.iter().map(|c| c.track + 1).max().unwrap_or(0);

    rsx! {
        div {
            class: "relative grid grid-cols-7 border-border/40 {border_b}",
            // Background day cells (numbers + click/drop targets).
            for (col, date) in props.days.iter().enumerate() {
                {
                    let date = *date;
                    let on_event = props.on_event;
                    let ctx = use_drag_context();
                    let mut bg = if date.month() == props.cur_month {
                        "bg-background"
                    } else {
                        "bg-background/40 text-muted-foreground"
                    };
                    if date == props.today {
                        bg = "bg-primary/10";
                    }
                    // Cyclic overlay — reset weeks + bonus weeks
                    // get a tint that wins over the in-month
                    // default but yields to the today highlight.
                    let kind = crate::cycle_overlay::kind_for_date(date);
                    let cycle_tint = if date == props.today {
                        ""
                    } else {
                        crate::cycle_overlay::cell_tint_class(kind)
                    };
                    let border_r = if col == 6 { "" } else { "border-r" };

                    rsx! {
                        div {
                            key: "{date}",
                            class: "border-border/40 {bg} {cycle_tint} {border_r} flex flex-col p-1 min-h-[6rem] hover:bg-accent/30 transition-colors",
                            // Click empty cell → create (mouse) /
                            // zoom to that day's Day view (touch).
                            onclick: move |_| {
                                if coarse {
                                    props.on_zoom_to_day.call(date);
                                    return;
                                }
                                if props.readonly { return; }
                                let start = day_start_utc(date) + chrono::Duration::hours(9);
                                let end = start + chrono::Duration::hours(1);
                                let event = CalendarEvent::new("New event", start, end);
                                on_event.call(CalendarMutation::Create { event });
                            },
                            // Drop dragged chip → reschedule by day delta.
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
                            // Day number
                            div { class: "flex items-center justify-end px-1",
                                span {
                                    class: if date == props.today {
                                        "text-xs font-semibold bg-primary text-primary-foreground rounded-full w-5 h-5 flex items-center justify-center"
                                    } else {
                                        "text-xs"
                                    },
                                    "{date.day()}"
                                }
                            }
                            // Reserve vertical space for the track
                            // chips (overlaid below). Add one more
                            // row when this cell has overflow so
                            // the "+N more" link doesn't escape.
                            {
                                let extra = if overflow_per_col[col] > 0 { 18 } else { 0 };
                                rsx! {
                                    div { class: "flex-1", style: "min-height: {tracks as i32 * 18 + extra}px;" }
                                }
                            }
                            // Overflow link
                            if overflow_per_col[col] > 0 {
                                {
                                    let n = overflow_per_col[col];
                                    let on_zoom = props.on_zoom_to_day;
                                    rsx! {
                                        button {
                                            r#type: "button",
                                            class: "text-[10px] text-muted-foreground hover:text-foreground hover:underline self-start px-1",
                                            onclick: move |e: MouseEvent| {
                                                e.stop_propagation();
                                                on_zoom.call(date);
                                            },
                                            "+{n} more"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Chip overlay — absolute layer covering the week row,
            // sitting above the cells but below the day-number
            // pills. Uses a 7-col grid so chips line up with cells.
            div {
                class: "absolute inset-x-0 top-6 grid grid-cols-7 gap-y-0.5 pointer-events-none px-0.5",
                for chip in visible_chips.iter() {
                    {
                        let chip = chip.clone();
                        let id = chip.event.id;
                        let on_open_editor = props.on_open_editor;
                        let on_event = props.on_event;
                        rsx! {
                            MonthChip {
                                key: "{id}-{chip.start_col}",
                                placement: chip,
                                readonly: props.readonly,
                                on_click: move |()| on_open_editor.call(id),
                                on_event,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct MonthChipProps {
    placement: MonthChipPlacement,
    readonly: bool,
    on_click: EventHandler<()>,
    on_event: EventHandler<CalendarMutation>,
}

#[component]
fn MonthChip(props: MonthChipProps) -> Element {
    let ctx = use_drag_context();
    let mut drag = ctx.state;
    // Coarse pointers: a ~18px chip is not a finger target, and the
    // HTML5 drag below is mouse-only anyway. Let the chip fall
    // through (`pointer-events: none` stays from the overlay layer)
    // so the tap hits the ≥44px day cell → Day view, where the chip
    // is full-size and draggable. Mouse keeps click-to-edit +
    // drag-to-reschedule.
    let coarse = *use_touch_context().coarse.read();
    let p = props.placement.clone();
    let event = p.event.clone();
    let event_id = event.id;
    let palette = chip_palette(event.color);
    let is_dragging = drag.read().as_ref().is_some_and(|d| d.event == event_id);
    let drag_style = if is_dragging { "opacity: 0.4;" } else { "" };
    let pe = if coarse { "" } else { "pointer-events-auto" };

    let track = p.track;
    let start_col = p.start_col;
    let span = p.span;
    let rounded_l = if p.continues_left {
        "rounded-l-none"
    } else {
        "rounded-l-md"
    };
    let rounded_r = if p.continues_right {
        "rounded-r-none"
    } else {
        "rounded-r-md"
    };
    let body = palette.body;
    let hover = palette.hover;
    // CSS grid is 1-indexed.
    let style = format!(
        "grid-column: {} / span {}; grid-row: {}; {drag_style}",
        start_col + 1,
        span,
        track + 1,
    );

    let on_click = props.on_click;

    rsx! {
        div {
            class: "truncate text-[11px] leading-4 font-semibold px-2 py-0.5 mx-px cursor-pointer select-none {pe} shadow-sm transition-colors {body} {hover} {rounded_l} {rounded_r}",
            style: "{style}",
            draggable: !props.readonly && !coarse,
            "data-cal-drag": "true",
            ondragstart: move |e: Event<DragData>| {
                if props.readonly { return; }
                let dt = e.data().data_transfer();
                let _ = dt.set_data(DT_MIME, &event_id.to_string());
                drag.set(Some(super::drag::DragState {
                    event: event_id,
                    kind: super::drag::DragKind::Move,
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
