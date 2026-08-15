//! Shared time-grid view used by week (`days.len() == 7`) and day
//! (`days.len() == 1`) variants.
//!
//! Pixel-based vertical layout so drop-y math is direct: the hour
//! height is derived from the scroll container (06:00–24:00 fills
//! the viewport, 28px/hour floor), measured on mount and
//! re-derived on viewport resize. Overlap column-splitting comes from
//! [`crate::layout::day_overlap_layout`]: events that share time
//! get split into sub-columns. Sweep-to-create on the column
//! background gives the Google-style click-and-drag event creation.

use std::rc::Rc;

use chrono::{Datelike, Duration, NaiveDate};
use dioxus::html::geometry::PixelsVector2D;
use dioxus::html::input_data::MouseButton;
use dioxus::html::{MountedData, ResizeData, ScrollBehavior};
use dioxus::prelude::*;

use crate::layout::{TimeBlockPlacement, day_overlap_layout};
use crate::store::CalendarMutation;
use crate::time::{day_start_utc, hour_labels};
use crate::types::{CalendarEvent, EventId, TemplateBlock};

use super::all_day_strip::AllDayStrip;
use super::drag::{
    BlockDrag, BlockDragKind, DRAG_THRESHOLD_PX, DragKind, Ghost, LONG_PRESS_MS,
    use_block_drag_context, use_drag_context, use_touch_context,
};
use super::event_chip::EventChip;
use super::now_line::NowLine;
use super::style::{chip_palette, template_palette};

/// Fallback row height when the scroll container can't be measured
/// (also the floor for very short viewports).
const PX_PER_HOUR_DEFAULT: i64 = 48;
const MIN_PX_PER_HOUR: i64 = 28;
const SNAP_MINUTES: i64 = 15;
/// Default visible day window: 06:00–24:00. The grid still spans
/// the whole 24h (00:00–06:00 is reachable by scrolling up — the
/// sleep block lives there) but gets no default screen space: the
/// hour height is derived from the container so the 18h window
/// exactly fills it, and the container starts scrolled to 06:00.
const WINDOW_START_HOUR: i64 = 6;
const WINDOW_HOURS: i64 = 18;

#[derive(Props, Clone, PartialEq)]
pub struct TimeGridViewProps {
    pub days: Vec<NaiveDate>,
    pub events: Vec<CalendarEvent>,
    /// Faded day-plan blocks drawn behind real events.
    #[props(default)]
    pub template_blocks: Vec<TemplateBlock>,
    /// Fired with `(date, block_id)` when a plan block is clicked.
    #[props(default)]
    pub on_block_click: Option<EventHandler<(NaiveDate, String)>>,
    /// Fired with `(date, block_id, start_min, end_min)` on a block drag.
    #[props(default)]
    pub on_block_edit: Option<EventHandler<crate::types::BlockEdit>>,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<CalendarMutation>,
    pub on_open_editor: EventHandler<EventId>,
}

#[component]
pub fn TimeGridView(props: TimeGridViewProps) -> Element {
    let today = chrono::Local::now().date_naive();
    // Hour height, derived from the scroll container's height so
    // 06:00–24:00 fills the default viewport exactly — measured on
    // mount and re-derived on every viewport resize (the container
    // carries an `onresize` ResizeObserver hook).
    let mut px_per_hour = use_signal(|| PX_PER_HOUR_DEFAULT);
    // The scroll container element, kept so the resize handler can
    // read + restore the scroll position.
    let mut scroller: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let pph = px_per_hour();
    let col_height_px = pph * 24;
    // Split: all-day events go to the top strip, timed events to
    // the time grid itself. `position` would mis-classify multi-day
    // timed events; we treat duration ≥ 24h with midnight-aligned
    // start as effectively all-day too, so a "Vacation" event the
    // user created via month view (start at 00:00, end exclusive)
    // also shows in the all-day strip.
    let (all_day_events, timed_events): (Vec<_>, Vec<_>) = props
        .events
        .iter()
        .cloned()
        .partition(|ev| ev.all_day || is_effectively_all_day(ev));

    rsx! {
        // `data-cal-grid` scopes the document-level touch-capture
        // release (see `drag::install_touch_capture_release`) so touch
        // drags hit-test like mouse drags inside the grid.
        div { class: "flex flex-col h-full w-full",
            "data-cal-grid": "true",
            // Fixed header block (day strip + all-day row). Reserves
            // the same scrollbar gutter as the scrollable body below
            // (`scrollbar-gutter: stable` on an overflow-hidden box)
            // so header columns stay pixel-aligned with the grid.
            div { class: "shrink-0 overflow-y-hidden [scrollbar-gutter:stable]",
            // Day header strip
            div { class: "grid border-b border-border/60",
                style: "grid-template-columns: 56px repeat({props.days.len()}, 1fr);",
                div {}
                for date in props.days.iter() {
                    {
                        let is_today = *date == today;
                        let day_name = date.format("%a").to_string();
                        let day_num = date.day();
                        let header_cycle_kind = crate::cycle_overlay::kind_for_date(*date);
                        let header_tint = if is_today {
                            "bg-primary/[0.04]"
                        } else {
                            crate::cycle_overlay::cell_tint_class(header_cycle_kind)
                        };
                        let cell_cls = format!(
                            "flex items-center justify-center gap-2 py-1.5 text-xs {header_tint}"
                        );
                        rsx! {
                            div {
                                key: "{date}",
                                class: "{cell_cls}",
                                span {
                                    class: if is_today {
                                        "text-primary font-semibold uppercase tracking-wider text-[10px]"
                                    } else {
                                        "text-muted-foreground uppercase tracking-wider text-[10px]"
                                    },
                                    "{day_name}"
                                }
                                span {
                                    class: if is_today {
                                        "font-semibold bg-primary text-primary-foreground rounded-full w-6 h-6 flex items-center justify-center text-sm"
                                    } else {
                                        "font-medium text-sm"
                                    },
                                    "{day_num}"
                                }
                            }
                        }
                    }
                }
            }
            // All-day strip
            AllDayStrip {
                days: props.days.clone(),
                events: all_day_events,
                readonly: props.readonly,
                on_event: props.on_event,
                on_open_editor: props.on_open_editor,
            }
            } // end fixed header block
            // Scrollable grid body. On mount: size the hour rows so
            // the 06:00–24:00 window fills the container, then start
            // scrolled to 06:00 (pre-06:00 stays reachable upward).
            // On resize: re-derive the row height and keep the
            // top-of-viewport hour anchored.
            div { class: "flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable]",
                onmounted: move |e: Event<MountedData>| {
                    scroller.set(Some(e.data()));
                    spawn(async move {
                        let mut p = PX_PER_HOUR_DEFAULT;
                        if let Ok(rect) = e.data().get_client_rect().await {
                            let h = rect.size.height;
                            if h > 100.0 {
                                p = ((h / WINDOW_HOURS as f64).floor() as i64)
                                    .max(MIN_PX_PER_HOUR);
                            }
                        }
                        px_per_hour.set(p);
                        let y = (WINDOW_START_HOUR * p) as f64;
                        let _ = e
                            .data()
                            .scroll(PixelsVector2D::new(0.0, y), ScrollBehavior::Instant)
                            .await;
                    });
                },
                // Viewport resize → re-derive the hour height (same
                // formula + MIN_PX_PER_HOUR floor as on mount) and
                // rescale the scroll offset so the hour at the top
                // of the viewport stays anchored (06:00 right after
                // mount).
                onresize: move |e: Event<ResizeData>| {
                    let Ok(size) = e.data().get_content_box_size() else { return };
                    let h = size.height;
                    if h <= 100.0 {
                        return;
                    }
                    let new_p = ((h / WINDOW_HOURS as f64).floor() as i64)
                        .max(MIN_PX_PER_HOUR);
                    let old_p = *px_per_hour.peek();
                    if new_p == old_p {
                        return;
                    }
                    let el = scroller.peek().clone();
                    spawn(async move {
                        let mut y = (WINDOW_START_HOUR * new_p) as f64;
                        if let Some(el) = el {
                            if let Ok(off) = el.get_scroll_offset().await {
                                y = off.y * new_p as f64 / old_p as f64;
                            }
                            px_per_hour.set(new_p);
                            let _ = el
                                .scroll(PixelsVector2D::new(0.0, y), ScrollBehavior::Instant)
                                .await;
                        } else {
                            px_per_hour.set(new_p);
                        }
                    });
                },
                div {
                    class: "grid relative",
                    style: "grid-template-columns: 56px repeat({props.days.len()}, 1fr); height: {col_height_px}px;",
                    HourAxis { px_per_hour: pph }
                    for (idx, date) in props.days.iter().enumerate() {
                        DayColumn {
                            key: "{date}",
                            date: *date,
                            placements: day_overlap_layout(*date, &timed_events),
                            template_blocks: props.template_blocks.clone(),
                            on_block_click: props.on_block_click,
                            on_block_edit: props.on_block_edit,
                            is_last: idx == props.days.len() - 1,
                            px_per_hour: pph,
                            readonly: props.readonly,
                            on_event: props.on_event,
                            on_open_editor: props.on_open_editor,
                        }
                    }
                    // Now-line overlay — placed after columns so
                    // it stacks on top.
                    NowLine { days: props.days.clone(), px_per_hour: pph }
                }
            }
        }
    }
}

/// Heuristic: a 24h+ duration event starting at midnight is
/// effectively an all-day event regardless of the `all_day` flag.
/// Lets month-view-created "Vacation" events render in the all-day
/// strip without forcing a data migration.
fn is_effectively_all_day(ev: &CalendarEvent) -> bool {
    let dur = (ev.end - ev.start).num_minutes();
    let starts_at_midnight = ev.start.time() == chrono::NaiveTime::MIN;
    dur >= 24 * 60 && starts_at_midnight
}

#[component]
fn HourAxis(px_per_hour: i64) -> Element {
    rsx! {
        div { class: "relative border-r border-border/60 text-[10px] text-muted-foreground/80 tabular-nums",
            for (h, label) in hour_labels() {
                div {
                    key: "{h}",
                    class: "absolute right-1 -translate-y-2",
                    style: "top: {i64::from(h) * px_per_hour}px;",
                    "{label}"
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct DayColumnProps {
    date: NaiveDate,
    placements: Vec<TimeBlockPlacement>,
    #[props(default)]
    template_blocks: Vec<TemplateBlock>,
    /// Fired with `(date, block_id)` when a plan block is clicked.
    #[props(default)]
    on_block_click: Option<EventHandler<(NaiveDate, String)>>,
    /// Fired with `(date, block_id, start_min, end_min)` after a block
    /// is dragged to a new time.
    #[props(default)]
    on_block_edit: Option<EventHandler<crate::types::BlockEdit>>,
    is_last: bool,
    px_per_hour: i64,
    readonly: bool,
    on_event: EventHandler<CalendarMutation>,
    on_open_editor: EventHandler<EventId>,
}

/// Sweep state: a `(start_min, current_min)` range the user is
/// dragging out with the primary mouse button. While `Some` the
/// column shows a ghost block; on `mouseup` it commits to a
/// `Create` mutation. Clamped to a single day — drag past
/// midnight just sticks at the day edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sweep {
    anchor_min: i64,
    current_min: i64,
}

impl Sweep {
    fn range(self) -> (i64, i64) {
        let a = self.anchor_min.min(self.current_min);
        let b = self.anchor_min.max(self.current_min);
        (
            snap_minutes(a),
            snap_minutes(b).max(snap_minutes(a) + SNAP_MINUTES),
        )
    }
}

#[component]
fn DayColumn(props: DayColumnProps) -> Element {
    let mut ctx = use_drag_context();
    let pph = props.px_per_hour;
    let date = props.date;
    let on_event = props.on_event;
    let on_open_editor = props.on_open_editor;
    let today = chrono::Local::now().date_naive();
    let is_today = date == today;
    let border_r = if props.is_last { "" } else { "border-r" };
    // Cyclic overlay — reset / bonus weeks paint the column.
    // Today's primary tint wins over the cycle tint.
    let cycle_kind = crate::cycle_overlay::kind_for_date(date);
    let column_bg = if is_today {
        "bg-primary/[0.04]"
    } else {
        crate::cycle_overlay::cell_tint_class(cycle_kind)
    };
    let mut sweep: Signal<Option<Sweep>> = use_signal(|| None);

    // Commit the in-flight sweep as a Create (a bare click yields a
    // 1-hour event at the clicked slot). Shared by pointerup (mouse:
    // the sweep's mousedown fired *before* pointerup) and click
    // (touch: the browser synthesizes mousedown/mouseup/click *after*
    // pointerup, so the sweep the synthetic mousedown armed would
    // otherwise linger as a stuck ghost — the trailing click commits
    // it, making tap-to-create work).
    let mut commit_sweep = move || {
        let Some(s) = sweep.take() else { return };
        let (start_min, end_min) = s.range();
        let start = day_start_utc(date) + Duration::minutes(start_min);
        let end = day_start_utc(date) + Duration::minutes(end_min);
        let end = if (end - start).num_minutes() <= SNAP_MINUTES {
            start + Duration::hours(1)
        } else {
            end
        };
        let event = CalendarEvent::new("New event", start, end);
        on_event.call(CalendarMutation::Create { event });
    };

    // Day-plan blocks for this column's date, pre-resolved to pixel
    // geometry. Faded, clickable placement guides.
    struct TplPlacement {
        id: String,
        start_min: i64,
        end_min: i64,
        top: i64,
        h: i64,
        pal: &'static str,
        label: String,
        assignment: Option<String>,
        soft: bool,
    }
    let template_placements: Vec<TplPlacement> = props
        .template_blocks
        .iter()
        .filter(|tb| tb.date == date && tb.end_min > tb.start_min)
        .map(|tb| TplPlacement {
            id: tb.id.clone(),
            start_min: i64::from(tb.start_min),
            end_min: i64::from(tb.end_min),
            top: i64::from(tb.start_min) * pph / 60,
            h: i64::from(tb.end_min - tb.start_min) * pph / 60,
            pal: template_palette(tb.color),
            label: tb.label.clone(),
            assignment: tb.assignment.clone(),
            soft: tb.soft,
        })
        .collect();
    let on_block_click = props.on_block_click;
    let on_block_edit = props.on_block_edit;
    let mut block_ctx = use_block_drag_context();
    // Pending block long-presses arm through the shared TouchContext
    // — disarm (slop / lift / cancel) lives at the Calendar root,
    // which sees every bubbled pointer event no matter which element
    // it hit-tests to (this column, a neighbor, or a chip).
    let mut touch = use_touch_context();
    let coarse = *touch.coarse.read();
    // The drag whose *target* day is this column (drives the ghost).
    let active_block = block_ctx
        .drag
        .read()
        .as_ref()
        .filter(|d| d.date == date && d.committed)
        .cloned();
    // The id of the block whose *origin* day is this column — dim it
    // while a (possibly cross-day) drag of it is in flight.
    let dragging_orig_id = block_ctx
        .drag
        .read()
        .as_ref()
        .filter(|d| d.orig_date == date && d.committed)
        .map(|d| d.block_id.clone());

    rsx! {
        div {
            class: "relative border-border/40 {border_r} {column_bg}",
            // Plan-block drag — handled at the column level so moves
            // bubble here regardless of which child the pointer is over.
            onpointermove: move |e: Event<PointerData>| {
                let snap = block_ctx.drag.peek().clone();
                let Some(mut bd) = snap else { return };
                // Resize stays on the block's own day; only a Move roams
                // across columns.
                let is_resize = !matches!(bd.kind, BlockDragKind::Move);
                if is_resize && bd.orig_date != date {
                    return;
                }
                let page_y = e.data().page_coordinates().y;
                if !bd.committed {
                    if (page_y - bd.start_page_y).abs() < DRAG_THRESHOLD_PX as f64 {
                        return;
                    }
                    bd.committed = true;
                }
                let col_y = e.data().element_coordinates().y as i64;
                let pointer_min = px_to_minutes(col_y, pph);
                match bd.kind {
                    BlockDragKind::Move => {
                        bd.date = date; // follow the cursor's column
                        let dur = bd.orig_end_min - bd.orig_start_min;
                        let s = snap_minutes(pointer_min - bd.grab_offset_min)
                            .clamp(0, (1440 - dur).max(0));
                        bd.cur_start_min = s;
                        bd.cur_end_min = s + dur;
                    }
                    BlockDragKind::ResizeEnd => {
                        bd.cur_start_min = bd.orig_start_min;
                        bd.cur_end_min =
                            snap_minutes(pointer_min).clamp(bd.orig_start_min + SNAP_MINUTES, 1440);
                    }
                    BlockDragKind::ResizeStart => {
                        bd.cur_start_min =
                            snap_minutes(pointer_min).clamp(0, bd.orig_end_min - SNAP_MINUTES);
                        bd.cur_end_min = bd.orig_end_min;
                    }
                }
                block_ctx.drag.set(Some(bd));
            },
            // Hour grid lines (solid).
            for h in 1..24u32 {
                div {
                    key: "h-{h}",
                    class: "absolute left-0 right-0 border-t border-border/30 pointer-events-none",
                    style: "top: {h as i64 * pph}px;",
                }
            }
            // Half-hour grid lines (dashed, very faint) — improves
            // legibility for 30/15-minute events.
            for h in 0..24u32 {
                div {
                    key: "h2-{h}",
                    class: "absolute left-0 right-0 border-t border-dashed border-border/15 pointer-events-none",
                    style: "top: {h as i64 * pph + pph / 2}px;",
                }
            }
            // Background surface — captures all pointer-based input
            // for this column (drag-tracking + sweep-to-create).
            // Sits BELOW the event blocks so chips win the initial
            // pointerdown; the chip body sets `pointer-events: none`
            // while it is being dragged so subsequent pointermoves
            // fall through to this surface as the user drags
            // across.
            // `pan-y` (not `none`): a finger on the empty grid scrolls
            // it. Drags that must beat scrolling either start from a
            // long-press / resize handle (committed → the Calendar
            // root's touchmove guard preventDefaults the pan) or from
            // a mouse, where touch-action is irrelevant.
            div {
                class: "absolute inset-0",
                style: "touch-action: pan-y;",
                // Snap-preview during an active drag. The drag isn't
                // "committed" until the pointer has moved past the
                // threshold — a pure click leaves DragState in place
                // but never updates the ghost, so onclick on the
                // chip wins.
                onpointermove: move |e: Event<PointerData>| {
                    if props.readonly { return; }
                    let snapshot = ctx.state.peek().clone();
                    let Some(mut ds) = snapshot else { return };
                    if !ds.committed {
                        let p = e.data().page_coordinates();
                        let dx = (p.x - ds.start_page_x).abs();
                        let dy = (p.y - ds.start_page_y).abs();
                        if dx < super::drag::DRAG_THRESHOLD_PX as f64
                            && dy < super::drag::DRAG_THRESHOLD_PX as f64
                        {
                            return;
                        }
                        ds.committed = true;
                        ctx.state.set(Some(ds.clone()));
                    }
                    let y = e.data().element_coordinates().y as i64;
                    let snap_min = snap_minutes(px_to_minutes(y, pph));
                    let (start_min, end_min) = ghost_range(ds.kind, ds.orig_start, ds.orig_end, snap_min, date);
                    ctx.ghost.set(Some(Ghost {
                        event: ds.event,
                        date,
                        start_min,
                        end_min,
                        color: ds.color,
                        title: ds.title.clone(),
                    }));
                },
                // Commit on pointerup. Either commits the drag or
                // commits a sweep-to-create — whichever is active.
                onpointerup: move |e: Event<PointerData>| {
                    if props.readonly { return; }
                    let drag_snapshot = ctx.state.peek().clone();
                    if let Some(ds) = drag_snapshot {
                        // Committed but never moved — a touch
                        // long-press lifted in place (mouse drags
                        // can't get here: their commit happens inside
                        // pointermove, which also sets the ghost).
                        // Dropping "at the finger" would snap the
                        // event's start to mid-chip, so leave it be.
                        if ctx.ghost.peek().is_none() {
                            ctx.state.set(None);
                            return;
                        }
                        let y = e.data().element_coordinates().y as i64;
                        let drop_min = snap_minutes(px_to_minutes(y, pph));
                        match ds.kind {
                            DragKind::Move => {
                                let new_start = day_start_utc(date) + Duration::minutes(drop_min);
                                let duration = ds.orig_end - ds.orig_start;
                                on_event.call(CalendarMutation::Reschedule {
                                    id: ds.event,
                                    start: new_start,
                                    end: new_start + duration,
                                });
                            }
                            DragKind::ResizeEnd => {
                                let origin_min_from_day = (ds.orig_start - day_start_utc(date)).num_minutes();
                                let min_end_min = origin_min_from_day + SNAP_MINUTES;
                                let new_end = day_start_utc(date)
                                    + Duration::minutes(drop_min.max(min_end_min));
                                on_event.call(CalendarMutation::Reschedule {
                                    id: ds.event,
                                    start: ds.orig_start,
                                    end: new_end,
                                });
                            }
                            DragKind::ResizeStart => {
                                let origin_end_min_from_day = (ds.orig_end - day_start_utc(date)).num_minutes();
                                let max_start_min = origin_end_min_from_day - SNAP_MINUTES;
                                let new_start = day_start_utc(date)
                                    + Duration::minutes(drop_min.min(max_start_min));
                                on_event.call(CalendarMutation::Reschedule {
                                    id: ds.event,
                                    start: new_start,
                                    end: ds.orig_end,
                                });
                            }
                        }
                        ctx.state.set(None);
                        ctx.ghost.set(None);
                        return;
                    }
                    // No drag in flight — commit sweep-to-create.
                    commit_sweep();
                },
                // Touch taps arm the sweep *after* pointerup (synthetic
                // mouse ordering) — commit it here so the tap creates
                // instead of leaving a stuck ghost.
                onclick: move |_| {
                    if props.readonly { return; }
                    commit_sweep();
                },
                // Sweep to create — primary button only. Click
                // without sweep falls through to the pointerup
                // branch and produces a 1-hour event at the
                // clicked slot.
                onmousedown: move |e: MouseEvent| {
                    if props.readonly { return; }
                    if e.data().trigger_button() != Some(MouseButton::Primary) { return; }
                    let y = e.data().element_coordinates().y as i64;
                    let anchor = snap_minutes(px_to_minutes(y, pph));
                    sweep.set(Some(Sweep { anchor_min: anchor, current_min: anchor }));
                },
                onmousemove: move |e: MouseEvent| {
                    if props.readonly { return; }
                    let Some(mut s) = *sweep.peek() else { return };
                    let y = e.data().element_coordinates().y as i64;
                    s.current_min = snap_minutes(px_to_minutes(y, pph));
                    sweep.set(Some(s));
                },
                onmouseleave: move |_| {
                    // Bail on the sweep if the pointer leaves the
                    // column — avoids ghost blocks lingering after
                    // a drag escapes the day. Drag-state survives
                    // because it belongs to DragContext, not the
                    // column's local sweep.
                    sweep.set(None);
                },
            }
            // Sweep ghost block — carries a live time-range label so
            // the gesture reads as "creating an event here" while the
            // user drags it out.
            if let Some(s) = *sweep.read() {
                {
                    let (a, b) = s.range();
                    let top = minutes_to_px(a, pph);
                    let h = minutes_to_px(b - a, pph).max(2);
                    let label = format_ghost_label(a, b);
                    rsx! {
                        div {
                            class: "absolute left-1 right-1 z-10 rounded-sm bg-primary/20 border border-primary/60 pointer-events-none overflow-hidden",
                            style: "top: {top}px; height: {h}px;",
                            if h >= 16 {
                                span { class: "block px-1.5 pt-px text-[10px] font-medium leading-4 text-primary truncate",
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
            // Day-plan blocks — clickable guides above the sweep
            // surface but behind real events. Clicking one asks the
            // consumer to edit that date's plan.
            for tp in template_placements.iter() {
                {
                    let block_id = tp.id.clone();
                    let down_id = tp.id.clone();
                    let rs_id = tp.id.clone();
                    let re_id = tp.id.clone();
                    let bstart = tp.start_min;
                    let bend = tp.end_min;
                    let draggable_block = on_block_edit.is_some();
                    let being_dragged = dragging_orig_id.as_deref() == Some(tp.id.as_str());
                    let interactive = on_block_click.is_some() || draggable_block;
                    let cursor = if draggable_block {
                        "cursor-grab active:cursor-grabbing hover:brightness-125"
                    } else if on_block_click.is_some() {
                        "cursor-pointer hover:brightness-125"
                    } else {
                        ""
                    };
                    // While this block is being dragged, dim it + let
                    // pointer events fall through to the column handler.
                    let pe = if interactive && !being_dragged { "" } else { "pointer-events-none" };
                    let dim = if being_dragged { "opacity-30" } else { "" };
                    // Soft (template-fallback) blocks read as a planned
                    // shape, not a confirmed block: dashed outline +
                    // reduced opacity. Explicit blocks keep the solid
                    // left-accent palette.
                    let softness = if tp.soft {
                        "border border-dashed border-border/70 opacity-60"
                    } else {
                        ""
                    };
                    // Build a drag for this block in `kind` mode.
                    let mk_drag = move |kind: BlockDragKind, id: String, grab: i64, page_y: f64| BlockDrag {
                        block_id: id,
                        kind,
                        orig_date: date,
                        date,
                        orig_start_min: bstart,
                        orig_end_min: bend,
                        grab_offset_min: grab,
                        cur_start_min: bstart,
                        cur_end_min: bend,
                        start_page_y: page_y,
                        committed: false,
                    };
                    rsx! {
                        div {
                            key: "tpl-{tp.id}-{tp.top}",
                            class: "absolute left-0.5 right-0.5 rounded-[3px] overflow-hidden transition {cursor} {pe} {dim} {tp.pal} {softness}",
                            style: "top: {tp.top}px; height: {tp.h}px;",
                            onpointerdown: move |e: Event<PointerData>| {
                                if !draggable_block {
                                    return;
                                }
                                let elem_y = e.data().element_coordinates().y as i64;
                                let p = e.data().page_coordinates();
                                let bd = mk_drag(
                                    BlockDragKind::Move,
                                    down_id.clone(),
                                    px_to_minutes(elem_y, pph),
                                    p.y,
                                );
                                if e.data().pointer_type() == "touch" {
                                    // Long-press to drag — same dance as
                                    // EventChip: a moving finger scrolls,
                                    // a held one grabs the block
                                    // (committed, so the root touchmove
                                    // guard stops the pan). Disarm is the
                                    // Calendar root's job.
                                    let generation = touch.arm(p.x, p.y);
                                    spawn(async move {
                                        dioxus_sdk_time::sleep(
                                            std::time::Duration::from_millis(LONG_PRESS_MS),
                                        )
                                        .await;
                                        if touch.still_armed(generation) {
                                            touch.disarm();
                                            block_ctx.drag.set(Some(BlockDrag {
                                                committed: true,
                                                ..bd.clone()
                                            }));
                                        }
                                    });
                                } else {
                                    block_ctx.drag.set(Some(bd));
                                }
                            },
                            onclick: move |_| {
                                if let Some(cb) = on_block_click {
                                    cb.call((date, block_id.clone()));
                                }
                            },
                            // Resize handles (top / bottom edges).
                            // Coarse pointers get a fatter grab zone
                            // + `touch-action: none` — an explicit
                            // affordance starts the resize without a
                            // long-press, and the browser must not
                            // pan mid-resize.
                            if draggable_block {
                                {
                                    let handle_h = if coarse { "h-4" } else { "h-1.5" };
                                    rsx! {
                                        div {
                                            class: "absolute inset-x-0 top-0 z-10 {handle_h} cursor-ns-resize",
                                            style: "touch-action: none;",
                                            onpointerdown: move |e: Event<PointerData>| {
                                                e.stop_propagation();
                                                let page_y = e.data().page_coordinates().y;
                                                block_ctx.drag.set(Some(mk_drag(
                                                    BlockDragKind::ResizeStart,
                                                    rs_id.clone(),
                                                    0,
                                                    page_y,
                                                )));
                                            },
                                        }
                                        div {
                                            class: "absolute inset-x-0 bottom-0 z-10 {handle_h} cursor-ns-resize",
                                            style: "touch-action: none;",
                                            onpointerdown: move |e: Event<PointerData>| {
                                                e.stop_propagation();
                                                let page_y = e.data().page_coordinates().y;
                                                block_ctx.drag.set(Some(mk_drag(
                                                    BlockDragKind::ResizeEnd,
                                                    re_id.clone(),
                                                    0,
                                                    page_y,
                                                )));
                                            },
                                        }
                                    }
                                }
                            }
                            span {
                                class: "block px-1.5 py-px text-[10px] font-medium leading-snug truncate",
                                "{tp.label}"
                            }
                            if let Some(a) = tp.assignment.as_ref() {
                                if tp.h >= 30 {
                                    span {
                                        class: "block px-1.5 text-[10px] leading-snug truncate opacity-75 italic",
                                        "{a}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Live snapped ghost of the block being dragged.
            if let Some(bd) = active_block.as_ref() {
                {
                    let top = minutes_to_px(bd.cur_start_min, pph);
                    let h = minutes_to_px(bd.cur_end_min - bd.cur_start_min, pph).max(2);
                    rsx! {
                        div {
                            class: "absolute left-0.5 right-0.5 z-20 rounded-md border-2 border-primary/70 bg-primary/15 pointer-events-none",
                            style: "top: {top}px; height: {h}px;",
                        }
                    }
                }
            }
            // Events on top — clickable above the sweep surface.
            for placement in props.placements.iter() {
                {
                    let id = placement.event.id;
                    let style = block_style(placement, pph);
                    let event = placement.event.clone();
                    rsx! {
                        EventChip {
                            key: "{id}",
                            event,
                            position_style: style,
                            readonly: props.readonly,
                            on_click: move |()| on_open_editor.call(id),
                        }
                    }
                }
            }
            // Snapped ghost — only renders when this column is the
            // current drop target.
            if let Some(g) = ctx.ghost.read().clone() {
                if g.date == date {
                    {
                        let palette = chip_palette(g.color);
                        let top = minutes_to_px(g.start_min, pph);
                        let h = minutes_to_px(g.end_min - g.start_min, pph).max(8);
                        let label = format_ghost_label(g.start_min, g.end_min);
                        let body = palette.body;
                        rsx! {
                            div {
                                class: "absolute left-1 right-1 rounded-md pointer-events-none z-30 shadow-lg ring-2 ring-white/30 px-2 py-1 overflow-hidden {body}",
                                style: "top: {top}px; height: {h}px;",
                                div { class: "text-[11px] leading-4 font-semibold truncate", "{g.title}" }
                                div { class: "text-[10px] leading-3 opacity-90 truncate", "{label}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn format_ghost_label(start_min: i64, end_min: i64) -> String {
    fn fmt(min: i64) -> String {
        let h = (min / 60).rem_euclid(24);
        let m = min.rem_euclid(60);
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
    format!("{} – {}", fmt(start_min), fmt(end_min))
}

/// Compute snapped `(start_min, end_min)` for a ghost preview given
/// the current drag kind and where the cursor is pointing
/// (`snap_min` is already 15-min snapped). For Move the drop point
/// becomes the new start; for resize the dragged edge moves and the
/// other edge stays put — clamped so the event keeps at least one
/// snap interval of length.
fn ghost_range(
    kind: DragKind,
    orig_start: chrono::DateTime<chrono::Utc>,
    orig_end: chrono::DateTime<chrono::Utc>,
    snap_min: i64,
    target_date: NaiveDate,
) -> (i64, i64) {
    let orig_start_min_from_day = (orig_start - day_start_utc(target_date)).num_minutes();
    let orig_end_min_from_day = (orig_end - day_start_utc(target_date)).num_minutes();
    let duration = (orig_end - orig_start).num_minutes();
    match kind {
        DragKind::Move => (snap_min, snap_min + duration),
        DragKind::ResizeEnd => {
            let min_end = orig_start_min_from_day + SNAP_MINUTES;
            (orig_start_min_from_day, snap_min.max(min_end))
        }
        DragKind::ResizeStart => {
            let max_start = orig_end_min_from_day - SNAP_MINUTES;
            (snap_min.min(max_start), orig_end_min_from_day)
        }
    }
}

/// Build the CSS `style` string for an event block — vertical
/// position + height come from the placement's `top_min` /
/// `height_min`, horizontal sub-column from `column` /
/// `cluster_size`. Inset by 2px on each side so adjacent columns
/// have a thin gutter (Google's look).
fn block_style(p: &TimeBlockPlacement, px_per_hour: i64) -> String {
    let top = minutes_to_px(p.top_min, px_per_hour);
    let h = minutes_to_px(p.height_min, px_per_hour);
    let width_pct = 100.0_f32 / f32::from(p.cluster_size);
    let left_pct = f32::from(p.column) * width_pct;
    // Inner gutter via percentage subtraction would over-shrink
    // narrow clusters; instead use calc() so each block loses a
    // fixed 3px of effective width regardless of column count.
    format!(
        "top: {top}px; height: {h}px; left: calc({left_pct:.4}% + 2px); width: calc({width_pct:.4}% - 3px);"
    )
}

fn minutes_to_px(min: i64, px_per_hour: i64) -> i64 {
    (min * px_per_hour) / 60
}

fn px_to_minutes(px: i64, px_per_hour: i64) -> i64 {
    (px * 60) / px_per_hour
}

fn snap_minutes(min: i64) -> i64 {
    (min / SNAP_MINUTES) * SNAP_MINUTES
}
