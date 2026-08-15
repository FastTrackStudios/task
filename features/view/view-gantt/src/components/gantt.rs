//! Root component — wires props into a [`GanttState`] signal, exposes
//! it via context, and renders the toolbar + grid (sidebar) + chart
//! (timeline) layout. Hosts the gantt-wide pointermove/pointerup
//! handlers so a drag started on a bar survives the pointer drifting
//! into the sidebar / toolbar / outside the chart pane.

use architect_ui::prelude::*;
use chrono::Duration;
use dioxus::prelude::*;

use crate::store::{GanttEvent, GanttState, SelectMode, apply, descendants_of};
use crate::time::add;
use crate::types::{
    GanttColumn, GanttLink, GanttTask, Marker, TaskType, ZoomConfig, default_columns,
};

use super::chart::Chart;
use super::context_menu::{ContextMenuContext, ContextMenuOverlay};
use super::drag::{DragContext, DragKind};
use super::editor::TaskEditor;
use super::grid::Grid;
use super::toolbar::Toolbar;

#[derive(Clone)]
pub struct GanttContext {
    pub state: Signal<GanttState>,
    pub on_event: EventHandler<GanttEvent>,
}

/// Snapshot of the active columns at this render. Children (Grid)
/// read it to lay out the sidebar.
#[derive(Clone, PartialEq)]
pub struct ColumnsContext(pub Vec<GanttColumn>);

/// Outer-body scroll state, populated on every `onscroll` event on
/// `#gantt-body`. Chart/Grid use this to cull rows outside the
/// visible window (virtualization).
#[derive(Clone, Copy)]
pub struct ScrollContext {
    pub scroll_top: Signal<f32>,
    pub viewport_h: Signal<f32>,
}

#[derive(Props, PartialEq, Clone)]
pub struct GanttProps {
    pub tasks: Vec<GanttTask>,
    #[props(default)]
    pub links: Vec<GanttLink>,
    #[props(default)]
    pub markers: Vec<Marker>,
    #[props(default)]
    pub zoom: Option<ZoomConfig>,
    /// Initial cell width in px. Default 60.
    #[props(default = 60.0)]
    pub cell_width: f32,
    /// Width of the left sidebar (grid) in px. Ignored when
    /// `columns` is supplied — the grid width becomes the sum of
    /// column widths in that case.
    #[props(default = 360.0)]
    pub grid_width: f32,
    /// Optional sidebar columns. Defaults to a single "Task" column
    /// (matches the prior single-column grid).
    #[props(default)]
    pub columns: Option<Vec<GanttColumn>>,
    /// If supplied, the gantt becomes "controlled" — apply your own
    /// mutation in this callback. If not, events are applied to the
    /// internal state via [`apply`].
    #[props(default)]
    pub on_event: Option<EventHandler<GanttEvent>>,
    /// Optional CSS class to merge onto the root container.
    #[props(default)]
    pub class: Option<String>,
    /// Disable all editing affordances (drag/resize/link/inline-edit).
    /// The chart still selects and zooms.
    #[props(default = false)]
    pub readonly: bool,
}

/// Drag threshold (px) — pointer movement under this is treated as
/// a click, not a drag. Mirrors svar's 20px gate.
const DRAG_THRESHOLD_PX: f32 = 4.0;

#[component]
pub fn Gantt(props: GanttProps) -> Element {
    let mut state = use_signal(|| {
        let mut s = GanttState {
            tasks: props.tasks.clone(),
            links: props.links.clone(),
            markers: props.markers.clone(),
            cell_width: props.cell_width,
            readonly: props.readonly,
            ..GanttState::default()
        };
        if let Some(z) = props.zoom.clone() {
            s.zoom = z;
        }
        s
    });

    // Sync incoming props → internal state SYNCHRONOUSLY during
    // render so children see the new data in the same frame.
    //
    // (An earlier version used `use_effect`, but effects fire after
    // render — so a drag-commit went `update tasks signal → clear
    // drag → render with OLD task position and no drag offset →
    // visible snap back → next frame, effect catches up`. Writing
    // here, gated by a peek-compare, avoids that one-frame lag and
    // doesn't recurse.)
    {
        let need_tasks = state.peek().tasks != props.tasks;
        let need_links = state.peek().links != props.links;
        let need_markers = state.peek().markers != props.markers;
        let need_ro = state.peek().readonly != props.readonly;
        if need_tasks || need_links || need_markers || need_ro {
            let mut s = state.write();
            if need_tasks {
                s.tasks = props.tasks.clone();
            }
            if need_links {
                s.links = props.links.clone();
            }
            if need_markers {
                s.markers = props.markers.clone();
            }
            if need_ro {
                s.readonly = props.readonly;
            }
        }
    }

    let on_event_prop = props.on_event;
    let dispatch = use_callback(move |ev: GanttEvent| {
        if let Some(h) = on_event_prop {
            h.call(ev);
        } else {
            apply(&mut state.write(), &ev);
        }
    });
    let on_event = EventHandler::new(move |ev| dispatch.call(ev));

    use_context_provider(|| GanttContext {
        state,
        on_event: on_event.clone(),
    });
    let drag_ctx = use_context_provider(|| DragContext {
        state: Signal::new(None),
        linking: Signal::new(None),
    });
    use_context_provider(|| ContextMenuContext {
        open: Signal::new(None),
    });
    // Scroll position of the outer y-scroll container, used by Chart
    // / Grid to cull off-screen rows. Initialized to a reasonable
    // viewport height that gets overwritten on the first onscroll.
    let scroll_ctx = use_context_provider(|| ScrollContext {
        scroll_top: Signal::new(0.0),
        viewport_h: Signal::new(800.0),
    });
    // Columns config — provided as a fresh value each render so
    // consumers can swap columns dynamically without forcing the
    // gantt to remount.
    let columns_value = props.columns.clone().unwrap_or_else(default_columns);
    use_context_provider(|| ColumnsContext(columns_value.clone()));

    // Sidebar width: sum of column widths when `columns` is set;
    // otherwise honour the `grid_width` prop.
    let grid_width = if props.columns.is_some() {
        columns_value.iter().map(|c| c.width).sum::<f32>().max(80.0)
    } else {
        props.grid_width
    };
    let class_extra = props.class.clone().unwrap_or_default();
    let editing = state.read().editing;
    let dragging_active = drag_ctx.state.read().is_some();
    let linking_active = drag_ctx.linking.read().is_some();

    // ---- Gantt-wide pointer move/up handlers ----
    let mut drag_state = drag_ctx.state;
    let on_event_pm = on_event.clone();
    let on_event_pu = on_event.clone();
    let mut linking_sig = drag_ctx.linking;

    let on_pointer_move = move |e: Event<PointerData>| {
        // Fast-bail when nothing is dragging — pointermove fires
        // 60+ times/sec just on hover, and an unconditional
        // `document::eval` on every event was the source of the
        // perceived lag. Touch the signal via `peek` so we don't
        // subscribe this handler to drag updates.
        if drag_state.peek().is_none() {
            return;
        }
        let mut ds = drag_state.write();
        let Some(st) = ds.as_mut() else { return };
        let page_x = e.data().page_coordinates().x as f32;
        let dx = page_x - st.start_x;
        st.dx = dx;
        let committed = if !st.committed && dx.abs() >= DRAG_THRESHOLD_PX {
            st.committed = true;
            true
        } else {
            st.committed
        };
        // Drop the write guard before the JS eval so any signal
        // listeners can re-render in parallel.
        drop(ds);
        if committed {
            // Auto-scroll the chart pane / body when the pointer
            // nears an edge — only while a committed drag is in
            // flight.
            let cx = e.data().client_coordinates().x as f32;
            let cy = e.data().client_coordinates().y as f32;
            let js = format!(
                "(function(){{
                    const pane = document.getElementById('gantt-chart-pane');
                    if (pane) {{
                        const r = pane.getBoundingClientRect();
                        if ({cx} < r.left + 40) pane.scrollBy({{ left: -8 }});
                        else if ({cx} > r.right - 40) pane.scrollBy({{ left: 8 }});
                    }}
                    const body = document.getElementById('gantt-body');
                    if (body) {{
                        const r2 = body.getBoundingClientRect();
                        if ({cy} < r2.top + 40) body.scrollBy({{ top: -8 }});
                        else if ({cy} > r2.bottom - 40) body.scrollBy({{ top: 8 }});
                    }}
                }})();"
            );
            dioxus::document::eval(&js);
        }
        let _ = &on_event_pm;
    };

    let on_pointer_up = move |_: Event<PointerData>| {
        let snapshot = drag_state.peek().clone();
        let Some(st) = snapshot else {
            return;
        };
        if !st.committed {
            drag_state.set(None);
            return;
        }
        // Convert dx → time delta. Keep it fractional so small drags
        // commit visibly (svar rounds to whole units; that felt wrong
        // here — bars appeared to "snap back" on tiny drags).
        let delta = {
            let s = state.peek();
            let grid = s.build_grid();
            let unit_secs = (add(grid.min_unit, grid.start, 1) - grid.start).num_seconds() as f64;
            let secs = (f64::from(st.dx) / f64::from(grid.min_unit_width)) * unit_secs;
            Duration::seconds(secs as i64)
        };
        // Dispatch the mutation FIRST so the consumer's task signal
        // updates in the same synchronous batch as the drag-state
        // clear below — keeps the bar at its new x without a flicker
        // through the original position.
        let s = state.peek();
        let dragged_type = s
            .tasks
            .iter()
            .find(|t| t.id == st.task_id)
            .map(|t| t.task_type);
        // Progress drags translate `dx` into a 0..1 fraction over
        // the bar's width at drag start. Fires a single
        // `UpdateProgress`.
        if st.kind == DragKind::Progress {
            drop(s);
            let pct = (st.orig_progress + st.dx / st.orig_w).clamp(0.0, 1.0);
            on_event_pu.call(GanttEvent::UpdateProgress {
                id: st.task_id,
                progress: pct,
            });
            drag_state.set(None);
            return;
        }
        // Multi-select drag: when the dragged bar is one of several
        // selected, shift the entire selection by `delta`.
        if st.kind == DragKind::Move && s.selected.len() > 1 && s.selected.contains(&st.task_id) {
            let ids: Vec<_> = s.selected.iter().copied().collect();
            let pairs: Vec<(uuid::Uuid, _, _)> = ids
                .iter()
                .filter_map(|id| {
                    s.tasks
                        .iter()
                        .find(|t| t.id == *id)
                        .map(|t| (t.id, t.start, t.end))
                })
                .collect();
            drop(s);
            for (id, start, end) in pairs {
                on_event_pu.call(GanttEvent::UpdateDates {
                    id,
                    start: start + delta,
                    end: end + delta,
                });
            }
            drag_state.set(None);
            return;
        }
        // For a summary bar a `Move` is a cascade: shift every
        // descendant by `delta`. The summary itself is derived from
        // its children, so no event for it is needed.
        if matches!(dragged_type, Some(TaskType::Summary)) && st.kind == DragKind::Move {
            let descendants = descendants_of(&s, st.task_id);
            // Snapshot each descendant's current dates so the delta
            // applies to consistent originals (orig_start/orig_end
            // on `st` only describes the summary itself).
            let pairs: Vec<(uuid::Uuid, _, _)> = descendants
                .iter()
                .filter_map(|id| {
                    s.tasks
                        .iter()
                        .find(|t| t.id == *id)
                        .map(|t| (t.id, t.start, t.end))
                })
                .collect();
            drop(s);
            for (id, start, end) in pairs {
                on_event_pu.call(GanttEvent::UpdateDates {
                    id,
                    start: start + delta,
                    end: end + delta,
                });
            }
        } else {
            drop(s);
            match st.kind {
                DragKind::Move => {
                    on_event_pu.call(GanttEvent::UpdateDates {
                        id: st.task_id,
                        start: st.orig_start + delta,
                        end: st.orig_end + delta,
                    });
                }
                DragKind::ResizeStart => {
                    let new_start = (st.orig_start + delta).min(st.orig_end - Duration::hours(1));
                    on_event_pu.call(GanttEvent::UpdateDates {
                        id: st.task_id,
                        start: new_start,
                        end: st.orig_end,
                    });
                }
                DragKind::ResizeEnd => {
                    let new_end = (st.orig_end + delta).max(st.orig_start + Duration::hours(1));
                    on_event_pu.call(GanttEvent::UpdateDates {
                        id: st.task_id,
                        start: st.orig_start,
                        end: new_end,
                    });
                }
                DragKind::Progress => unreachable!("Progress drag handled above"),
            }
        }
        drag_state.set(None);
    };

    // Keyboard hotkeys. The root has `tabindex=-1` so click-to-
    // focus works. Routing:
    //   Esc          → cancel pending link / clear selection
    //   Del/Backspace → delete every selected task
    //   Enter        → open editor on the first selected
    //   ←/→          → nudge selected tasks by ±1 min-unit
    //   Ctrl/Meta+A  → select all
    let on_event_kb = on_event.clone();
    let on_key = move |e: Event<KeyboardData>| {
        let key = e.data().key();
        let mods = e.data().modifiers();
        match key {
            Key::Escape => {
                if linking_sig.peek().is_some() {
                    linking_sig.set(None);
                } else {
                    on_event_kb.call(GanttEvent::Select {
                        id: None,
                        mode: SelectMode::Clear,
                    });
                }
            }
            Key::Delete | Key::Backspace => {
                if state.read().readonly {
                    return;
                }
                let ids: Vec<_> = state.peek().selected.iter().copied().collect();
                for id in ids {
                    on_event_kb.call(GanttEvent::DeleteTask { id });
                }
            }
            Key::Enter => {
                if let Some(id) = state.peek().selected.iter().next().copied() {
                    on_event_kb.call(GanttEvent::OpenEditor { id });
                }
            }
            Key::ArrowLeft | Key::ArrowRight => {
                if state.read().readonly {
                    return;
                }
                let sign: i64 = if key == Key::ArrowLeft { -1 } else { 1 };
                let s = state.peek();
                let grid = s.build_grid();
                let unit_secs =
                    (add(grid.min_unit, grid.start, 1) - grid.start).num_seconds() as i64;
                let delta = Duration::seconds(sign * unit_secs);
                let pairs: Vec<_> = s
                    .selected
                    .iter()
                    .filter_map(|id| {
                        s.tasks
                            .iter()
                            .find(|t| t.id == *id)
                            .map(|t| (t.id, t.start, t.end))
                    })
                    .collect();
                drop(s);
                for (id, start, end) in pairs {
                    on_event_kb.call(GanttEvent::UpdateDates {
                        id,
                        start: start + delta,
                        end: end + delta,
                    });
                }
                e.prevent_default();
            }
            Key::Character(c) if (c == "a" || c == "A") && (mods.ctrl() || mods.meta()) => {
                // Replace selection with every task.
                let ids: Vec<_> = state.peek().tasks.iter().map(|t| t.id).collect();
                for (i, id) in ids.into_iter().enumerate() {
                    on_event_kb.call(GanttEvent::Select {
                        id: Some(id),
                        mode: if i == 0 {
                            SelectMode::Replace
                        } else {
                            SelectMode::Toggle
                        },
                    });
                }
                e.prevent_default();
            }
            _ => {}
        }
    };

    let root_class = format!(
        "gantt-root flex flex-col h-full w-full bg-background text-foreground border border-border rounded-md overflow-hidden outline-none{}{}{}",
        if dragging_active {
            " gantt-dragging"
        } else {
            ""
        },
        if linking_active { " gantt-linking" } else { "" },
        if class_extra.is_empty() {
            String::new()
        } else {
            format!(" {class_extra}")
        }
    );

    rsx! {
        div {
            class: "{root_class}",
            tabindex: "-1",
            style: "touch-action: none;",
            onpointermove: on_pointer_move,
            onpointerup: on_pointer_up,
            onkeydown: on_key,
            Toolbar {}
            // Outer body owns the vertical scroll. Sidebar and chart
            // are both children of this y-scroll so they slide in
            // lockstep — no manual `scrollTop` sync needed. The
            // chart pane handles its own horizontal scroll.
            div { class: "flex-1 min-h-0 overflow-y-auto flex flex-row",
                id: "gantt-body",
                onscroll: move |e: Event<ScrollData>| {
                    let mut top = scroll_ctx.scroll_top;
                    let mut vh = scroll_ctx.viewport_h;
                    top.set(e.data().scroll_top() as f32);
                    let h = e.data().client_height() as f32;
                    if h > 0.0 {
                        vh.set(h);
                    }
                },
                if props.tasks.is_empty() {
                    div { class: "flex-1 flex items-center justify-center p-8",
                        EmptyState { message: "No tasks yet. Add a task to see it on the timeline." }
                    }
                } else {
                    div {
                        class: "flex-none border-r border-border bg-card",
                        style: "width: {grid_width}px;",
                        Grid {}
                    }
                    div {
                        class: "flex-1 min-w-0 overflow-x-auto",
                        id: "gantt-chart-pane",
                        Chart {}
                    }
                }
            }
            if let Some(id) = editing {
                TaskEditor { task_id: id }
            }
            ContextMenuOverlay {}
        }
    }
}
