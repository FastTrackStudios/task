//! Root Calendar component — owns toolbar state (anchor date +
//! view mode + editor open state), provides drag context, dispatches
//! `CalendarMutation`s upward, and swaps in the right view.

use chrono::{Days, Months, NaiveDate};
use dioxus::prelude::*;

use crate::recurrence::expand_all;
use crate::store::CalendarMutation;
use crate::time::{day_start_utc, month_grid, week_days, week_start};
use crate::types::{CalendarEvent, EventId, TemplateBlock, ViewMode};

use super::day_view::DayView;
use super::drag::DragContext;
use super::event_editor::EventEditor;
use super::month_view::MonthView;
use super::slot_grid::{SlotGrid, SlotItem, SlotRow};
use super::toolbar::Toolbar;
use super::week_view::WeekView;

#[derive(Props, Clone, PartialEq)]
pub struct CalendarProps {
    pub events: Vec<CalendarEvent>,
    /// Defaults to today.
    #[props(default)]
    pub initial_anchor: Option<NaiveDate>,
    /// Defaults to `ViewMode::Week`.
    #[props(default)]
    pub initial_view: Option<ViewMode>,
    /// Day-plan blocks drawn behind real events in the week/day grid
    /// (per-date placement guides). Empty = no overlay. Not shown in
    /// month view.
    #[props(default)]
    pub template_blocks: Vec<TemplateBlock>,
    /// Fired with `(date, block_id)` when a plan block is clicked, so
    /// the consumer can edit that date's plan.
    #[props(default)]
    pub on_block_click: Option<EventHandler<(NaiveDate, String)>>,
    /// Fired with `(date, block_id, start_min, end_min)` when a plan
    /// block is dragged to a new time on the grid.
    #[props(default)]
    pub on_block_edit: Option<EventHandler<crate::types::BlockEdit>>,
    /// Fired with the visible `(first_date, last_date)` whenever the
    /// view range changes, so the consumer can load that range's plans.
    #[props(default)]
    pub on_range: Option<EventHandler<(NaiveDate, NaiveDate)>>,
    /// Optional muted context line for the toolbar (wide screens).
    #[props(default)]
    pub summary: Option<String>,

    // ── Block axis ──────────────────────────────────────────────
    // Supplying rows is what turns the Blocks mode on: a calendar with
    // no categories to show shouldn't offer a view that renders empty.
    /// Rows for [`ViewMode::Blocks`], in display order.
    #[props(default)]
    pub slot_rows: Vec<SlotRow>,
    /// What sits in the block grid. Kept separate from `events` because
    /// the two axes answer different questions — an item with no clock
    /// value has nothing to show on the time grid, and vice versa.
    #[props(default)]
    pub slot_items: Vec<SlotItem>,
    /// A block item was clicked, by [`SlotItem::id`].
    #[props(default)]
    pub on_slot_item: Option<EventHandler<String>>,
    /// An empty part of a block cell was clicked — `(date, row key)`.
    #[props(default)]
    pub on_slot_cell: Option<EventHandler<(NaiveDate, String)>>,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<CalendarMutation>,
}

#[component]
pub fn Calendar(props: CalendarProps) -> Element {
    let today = chrono::Local::now().date_naive();
    let mut anchor = use_signal(|| props.initial_anchor.unwrap_or(today));
    let mut view = use_signal(|| props.initial_view.unwrap_or_default());
    let mut editing: Signal<Option<EventId>> = use_signal(|| None);

    use_context_provider(|| DragContext {
        state: Signal::new(None),
        ghost: Signal::new(None),
    });
    use_context_provider(|| super::drag::BlockDragContext {
        drag: Signal::new(None),
    });
    let touch_ctx = use_context_provider(|| super::drag::TouchContext {
        coarse: Signal::new(false),
        lp_pending: Signal::new(None),
        lp_gen: Signal::new(0),
    });
    use_hook(super::drag::install_drag_image_suppressor);
    use_hook(super::drag::install_touch_capture_release);

    // Coarse-pointer probe — one-shot at mount, same shape as the
    // viewport probe below. Only ever flips *to* coarse, so a failed
    // or absent probe leaves the mouse behavior intact.
    use_hook(move || {
        let mut coarse = touch_ctx.coarse;
        spawn(async move {
            let mut probe = dioxus::document::eval(
                "dioxus.send(window.matchMedia('(pointer: coarse)').matches)",
            );
            if let Ok(true) = probe.recv::<bool>().await {
                coarse.set(true);
            }
        });
    });

    // Phones can't fit the 7-column week grid — start in Day view on
    // small screens. One-shot probe at mount that only ever switches
    // *to* Day, so desktop keeps its `initial_view` and a failed/absent
    // probe is a harmless no-op (the requested view stands).
    use_hook(move || {
        spawn(async move {
            let mut probe = dioxus::document::eval(
                "dioxus.send(window.matchMedia('(max-width: 640px)').matches)",
            );
            if let Ok(true) = probe.recv::<bool>().await {
                view.set(ViewMode::Day);
            }
        });
    });

    // Tell the consumer which dates are visible so it can load that
    // range's day plans. Fires on mount + every anchor/view change.
    let on_range = props.on_range;
    use_effect(move || {
        if let Some(cb) = on_range {
            let (s, e) = visible_dates(*anchor.read(), *view.read());
            cb.call((s, e));
        }
    });

    let on_event = props.on_event;
    let events = props.events.clone();

    // Expand any recurring masters to instances inside the visible
    // range so the views stay recurrence-agnostic. Buffered by one
    // day on each side so multi-day chips that start *just* before
    // the visible window still render with the correct edge.
    let (vis_start, vis_end) = visible_range(*anchor.read(), *view.read());
    let events_for_view = expand_all(&events, vis_start, vis_end);

    // Editor opens against the master (so series edits apply
    // globally for v1) — look up by id, not by expanded instance.
    let selected = editing
        .read()
        .and_then(|id| events.iter().find(|e| e.id == id).cloned());

    // Keyboard shortcuts — only fire when no editor sheet is open
    // (otherwise typing in the title input would jump views). The
    // outer div takes focus on mount; arrow keys / single-letter
    // shortcuts work without an explicit click.
    let editor_open = editing.read().is_some();
    let on_keydown = move |e: KeyboardEvent| {
        if editor_open {
            return;
        }
        let key = e.data().key();
        match key {
            Key::Character(ref s) => {
                let s = s.to_ascii_lowercase();
                match s.as_str() {
                    "t" => anchor.set(today),
                    "1" => view.set(ViewMode::Day),
                    "2" => view.set(ViewMode::Week),
                    "3" => view.set(ViewMode::Month),
                    "n" => {
                        if !props.readonly {
                            // Create on the *visible* anchor date (not
                            // today) so the new event lands in view.
                            let start = day_start_utc(*anchor.peek()) + chrono::Duration::hours(9);
                            let end = start + chrono::Duration::hours(1);
                            let event = CalendarEvent::new("New event", start, end);
                            let id = event.id;
                            on_event.call(CalendarMutation::Create { event });
                            editing.set(Some(id));
                        }
                    }
                    _ => {}
                }
            }
            Key::ArrowLeft => anchor.with_mut(|d| *d = step(*d, *view.read(), -1)),
            Key::ArrowRight => anchor.with_mut(|d| *d = step(*d, *view.read(), 1)),
            _ => {}
        }
    };

    // Root-level pointerup commits any drag-in-flight. Per-column
    // pointermove updates the ghost as the cursor moves; when the
    // user releases — even outside any column (e.g. past midnight,
    // off the grid) — this handler reads the last-known ghost and
    // commits a single `Reschedule`. Avoids the dead-zone bug where
    // pointerup outside the column never fired.
    let drag_ctx = super::drag::use_drag_context();
    let mut block_ctx = super::drag::use_block_drag_context();
    let on_block_edit = props.on_block_edit;
    let on_event_up = on_event;
    let mut lp_touch = touch_ctx;
    let on_pointer_up = move |_: Event<PointerData>| {
        // Disarm any pending long-press FIRST. This is the backstop
        // the arming chip/block can't provide: with implicit capture
        // released, their own pointerup may hit-test to a different
        // element entirely — only the root reliably sees the bubbled
        // event. Without this a stale 400ms timer commits a drag for
        // a finger that already lifted.
        lp_touch.disarm();
        // Plan-block drag commit takes priority — it's a separate
        // gesture from event drags.
        let block_snap = block_ctx.drag.peek().clone();
        if let Some(bd) = block_snap {
            block_ctx.drag.set(None);
            if bd.committed {
                // A long-press lifted in place never moved the block
                // — cur_* still equal the origin. Don't persist a
                // no-op edit (same guard the chip path has via its
                // never-set ghost).
                let unchanged = bd.date == bd.orig_date
                    && bd.cur_start_min == bd.orig_start_min
                    && bd.cur_end_min == bd.orig_end_min;
                if let Some(cb) = on_block_edit {
                    if !unchanged {
                        let s = bd.cur_start_min.clamp(0, 1440) as u16;
                        let e = bd.cur_end_min.clamp(0, 1440) as u16;
                        cb.call((bd.orig_date, bd.date, bd.block_id, s, e));
                    }
                }
                return;
            }
        }
        let drag_snap = drag_ctx.state.peek().clone();
        let Some(ds) = drag_snap else { return };
        // Click without movement — leave the event alone so the
        // chip's onclick can open the editor.
        if !ds.committed {
            drag_ctx.state.clone().set(None);
            drag_ctx.ghost.clone().set(None);
            return;
        }
        if let Some(g) = drag_ctx.ghost.peek().clone() {
            let new_start = day_start_utc(g.date) + chrono::Duration::minutes(g.start_min);
            let new_end = day_start_utc(g.date) + chrono::Duration::minutes(g.end_min);
            on_event_up.call(CalendarMutation::Reschedule {
                id: g.event,
                start: new_start,
                end: new_end,
            });
        }
        drag_ctx.state.clone().set(None);
        drag_ctx.ghost.clone().set(None);
    };

    // The browser fires pointercancel when it claims the gesture for
    // itself (a pan/scroll won the race, the tab lost focus, …) — the
    // drag can never complete, so drop it instead of leaving a stuck
    // ghost + faded chip.
    let mut cancel_block_ctx = block_ctx;
    let on_pointer_cancel = move |_: Event<PointerData>| {
        lp_touch.disarm();
        cancel_block_ctx.drag.set(None);
        drag_ctx.state.clone().set(None);
        drag_ctx.ghost.clone().set(None);
    };

    // Slop backstop for the pending long-press: once a scroll starts,
    // the moves hit-test to whatever is under the finger — possibly
    // never the arming chip/block — but they always bubble here.
    let on_pointer_move = move |e: Event<PointerData>| {
        let p = e.data().page_coordinates();
        lp_touch.disarm_if_strayed(p.x, p.y);
    };

    // While a drag is committed, eat touchmove so the browser can't
    // start scrolling mid-drag. The grid keeps `touch-action: pan-y`
    // (a moving finger scrolls); a *committed* drag only exists after
    // a long-press or a resize-handle grab, and from then on the
    // finger must track the event, not the viewport. preventDefault
    // on a non-passive touchmove is the only way to flip that choice
    // after the gesture has already started.
    let on_touch_move = move |e: Event<TouchData>| {
        let dragging = drag_ctx.state.peek().as_ref().is_some_and(|d| d.committed)
            || block_ctx.drag.peek().as_ref().is_some_and(|d| d.committed);
        if dragging {
            e.prevent_default();
        }
    };

    // Offer the block axis only when there are rows to put on it.
    let block_modes: Vec<ViewMode> = if props.slot_rows.is_empty() {
        vec![ViewMode::Day, ViewMode::Week, ViewMode::Month]
    } else {
        vec![
            ViewMode::Day,
            ViewMode::Week,
            ViewMode::Month,
            ViewMode::Blocks,
        ]
    };

    rsx! {
        div {
            class: "flex flex-col h-full w-full outline-none",
            tabindex: 0,
            autofocus: true,
            onkeydown: on_keydown,
            onpointerup: on_pointer_up,
            onpointercancel: on_pointer_cancel,
            onpointermove: on_pointer_move,
            ontouchmove: on_touch_move,
            Toolbar {
                anchor: *anchor.read(),
                view: *view.read(),
                on_prev: move |()| anchor.with_mut(|d| *d = step(*d, *view.read(), -1)),
                on_next: move |()| anchor.with_mut(|d| *d = step(*d, *view.read(), 1)),
                on_today: move |()| anchor.set(today),
                modes: block_modes.clone(),
                on_view_change: move |v: ViewMode| view.set(v),
                on_create: move |()| {
                    // Anchor date, not today — "+ New event" while
                    // browsing another week must create *there*.
                    let start = day_start_utc(*anchor.peek()) + chrono::Duration::hours(9);
                    let end = start + chrono::Duration::hours(1);
                    let event = CalendarEvent::new("New event", start, end);
                    let id = event.id;
                    on_event.call(CalendarMutation::Create { event });
                    editing.set(Some(id));
                },
                summary: props.summary.clone(),
                readonly: props.readonly,
            }
            div { class: "flex-1 min-h-0",
                match *view.read() {
                    ViewMode::Month => rsx! {
                        MonthView {
                            anchor: *anchor.read(),
                            events: events_for_view,
                            readonly: props.readonly,
                            on_event,
                            on_open_editor: move |id| editing.set(Some(id)),
                            on_zoom_to_day: move |d: NaiveDate| {
                                anchor.set(d);
                                view.set(ViewMode::Day);
                            },
                        }
                    },
                    ViewMode::Week => rsx! {
                        WeekView {
                            anchor: *anchor.read(),
                            events: events_for_view,
                            template_blocks: props.template_blocks.clone(),
                            on_block_click: props.on_block_click,
                            on_block_edit: props.on_block_edit,
                            readonly: props.readonly,
                            on_event,
                            on_open_editor: move |id| editing.set(Some(id)),
                        }
                    },
                    ViewMode::Blocks => rsx! {
                        SlotGrid {
                            days: week_days(*anchor.read()).to_vec(),
                            rows: props.slot_rows.clone(),
                            items: props.slot_items.clone(),
                            today: Some(chrono::Local::now().date_naive()),
                            on_item: move |id: String| {
                                if let Some(h) = &props.on_slot_item {
                                    h.call(id);
                                }
                            },
                            on_cell: move |cell: (NaiveDate, String)| {
                                if let Some(h) = &props.on_slot_cell {
                                    h.call(cell);
                                }
                            },
                        }
                    },
                    ViewMode::Day => rsx! {
                        DayView {
                            anchor: *anchor.read(),
                            events: events_for_view,
                            template_blocks: props.template_blocks.clone(),
                            on_block_click: props.on_block_click,
                            on_block_edit: props.on_block_edit,
                            readonly: props.readonly,
                            on_event,
                            on_open_editor: move |id| editing.set(Some(id)),
                        }
                    },
                }
            }
            if let Some(ev) = selected {
                EventEditor {
                    event: ev,
                    open: true,
                    on_close: move |()| editing.set(None),
                    on_event,
                }
            }
        }
    }
}

/// The UTC `[start, end)` window covered by the current view, with
/// a one-day padding on each side so chips that lap the edge still
/// render correctly.
/// The inclusive `(first, last)` calendar dates the current view
/// shows — what the consumer loads day plans for.
fn visible_dates(anchor: NaiveDate, view: ViewMode) -> (NaiveDate, NaiveDate) {
    match view {
        ViewMode::Month => {
            let grid = month_grid(anchor);
            (grid[0][0], grid[5][6])
        }
        ViewMode::Week => {
            let days = week_days(anchor);
            (days[0], days[6])
        }
        ViewMode::Day => (anchor, anchor),
        // Same span as the week it mirrors.
        ViewMode::Blocks => {
            let days = week_days(anchor);
            (days[0], days[6])
        }
    }
}

fn visible_range(
    anchor: NaiveDate,
    view: ViewMode,
) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
    let (start, end) = match view {
        ViewMode::Month => {
            let grid = month_grid(anchor);
            (grid[0][0], grid[5][6] + Days::new(1))
        }
        ViewMode::Week | ViewMode::Blocks => {
            let days = week_days(anchor);
            (days[0], days[6] + Days::new(1))
        }
        ViewMode::Day => (anchor, anchor + Days::new(1)),
    };
    (
        day_start_utc(start) - chrono::Duration::days(1),
        day_start_utc(end) + chrono::Duration::days(1),
    )
}

/// Step `anchor` by `dir` (-1 = prev, +1 = next) in the unit
/// matching `view`.
fn step(anchor: NaiveDate, view: ViewMode, dir: i64) -> NaiveDate {
    match view {
        ViewMode::Month => {
            if dir < 0 {
                anchor
                    .checked_sub_months(Months::new((-dir) as u32))
                    .unwrap_or(anchor)
            } else {
                anchor
                    .checked_add_months(Months::new(dir as u32))
                    .unwrap_or(anchor)
            }
        }
        // Blocks spans a week, so it pages by one.
        ViewMode::Week | ViewMode::Blocks => {
            let base = week_start(anchor);
            if dir < 0 {
                base.checked_sub_days(Days::new((-dir * 7) as u64))
                    .unwrap_or(base)
            } else {
                base.checked_add_days(Days::new((dir * 7) as u64))
                    .unwrap_or(base)
            }
        }
        ViewMode::Day => {
            if dir < 0 {
                anchor
                    .checked_sub_days(Days::new((-dir) as u64))
                    .unwrap_or(anchor)
            } else {
                anchor
                    .checked_add_days(Days::new(dir as u64))
                    .unwrap_or(anchor)
            }
        }
    }
}
