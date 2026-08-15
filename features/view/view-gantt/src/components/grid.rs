//! Left sidebar — text grid showing the task hierarchy.
//!
//! Click a row to select. Click the chevron on a summary task to
//! toggle children visibility. Double-click the name to open the
//! side editor. Drag a row vertically onto another to reorder
//! siblings (or reparent — dropping on the right-third of a row
//! makes it a child of the target).

use dioxus::prelude::*;
use architect_ui::prelude::*;
use uuid::Uuid;

use crate::store::GanttEvent;
use crate::types::{ColumnKind, GanttColumn, GanttTask, TaskType};

use super::gantt::{ColumnsContext, GanttContext, ScrollContext};

#[component]
pub fn Grid() -> Element {
    let ctx = use_context::<GanttContext>();
    let state = ctx.state;
    let on_event = ctx.on_event.clone();

    let s = state.read();
    let grid = s.build_grid();
    let (rows, _links) = s.layout(&grid);
    let row_h = s.row_height;
    let scale_rows = grid.rows.len();
    let header_h = row_h * scale_rows as f32;
    let selected = s.selected.clone();
    let readonly = s.readonly;
    drop(s);

    // Drag state — which row id is being dragged and what the
    // currently-hovered drop target looks like. The drop indicator
    // is rendered into the visible row layout via these.
    let mut dragging_id: Signal<Option<Uuid>> = use_signal(|| None);
    let mut drop_target: Signal<Option<(Uuid, DropMode)>> = use_signal(|| None);

    // Virtualization — cull rows outside the visible viewport,
    // replacing them with top/bottom spacers so the scrollbar
    // geometry and per-row alignment with the chart stay correct.
    let scroll = use_context::<ScrollContext>();
    let st = *scroll.scroll_top.read();
    let vh = *scroll.viewport_h.read();
    let buffer = 400.0_f32;
    let visible_top = ((st - header_h - buffer) / row_h).floor().max(0.0) as usize;
    let visible_bot_unclamped = ((st + vh - header_h + buffer) / row_h).ceil().max(0.0) as usize;
    let visible_bot = visible_bot_unclamped.min(rows.len());
    let visible_top = visible_top.min(visible_bot);
    let top_spacer = visible_top as f32 * row_h;
    let bot_spacer = (rows.len().saturating_sub(visible_bot)) as f32 * row_h;
    let rows: Vec<_> = rows
        .into_iter()
        .skip(visible_top)
        .take(visible_bot - visible_top)
        .collect();

    let columns = use_context::<ColumnsContext>().0;
    let header_columns = columns.clone();

    rsx! {
        div { class: "flex flex-col",
            div {
                class: "flex-none flex border-b border-border bg-card sticky top-0 z-20",
                style: "height: {header_h}px;",
                for col in header_columns.iter() {
                    div {
                        key: "h-{col.kind:?}",
                        class: "flex-none flex items-center px-3 border-r border-border/40",
                        style: "width: {col.width}px;",
                        Text { variant: TextVariant::Muted, "{col.label}" }
                    }
                }
            }
            div {
                if top_spacer > 0.0 {
                    div { style: "height: {top_spacer}px;" }
                }
                for r in rows.into_iter() {
                    GridRow {
                        key: "{r.task.id}",
                        task: r.task.clone(),
                        level: r.level,
                        has_children: r.has_children,
                        height: row_h,
                        selected: selected.contains(&r.task.id),
                        columns: columns.clone(),
                        readonly,
                        dragging_id,
                        drop_target,
                        on_select: {
                            let on_event = on_event.clone();
                            let id = r.task.id;
                            EventHandler::new(move |()| on_event.call(GanttEvent::Select { id: Some(id), mode: crate::store::SelectMode::Replace }))
                        },
                        on_toggle: {
                            let on_event = on_event.clone();
                            let id = r.task.id;
                            EventHandler::new(move |()| on_event.call(GanttEvent::ToggleOpen { id }))
                        },
                        on_open_editor: {
                            let on_event = on_event.clone();
                            let id = r.task.id;
                            EventHandler::new(move |()| on_event.call(GanttEvent::OpenEditor { id }))
                        },
                        on_drop: {
                            let on_event = on_event.clone();
                            EventHandler::new(move |evt: DropEvent| {
                                let DropEvent {
                                    source_id,
                                    target_id,
                                    target_parent,
                                    mode,
                                } = evt;
                                let (new_parent, before) = match mode {
                                    DropMode::Before => (target_parent, Some(target_id)),
                                    DropMode::After => (target_parent, None_after(target_id)),
                                    DropMode::Child => (Some(target_id), None),
                                };
                                on_event.call(GanttEvent::ReorderTask {
                                    id: source_id,
                                    before,
                                    new_parent,
                                });
                                dragging_id.set(None);
                                drop_target.set(None);
                            })
                        },
                    }
                }
                if bot_spacer > 0.0 {
                    div { style: "height: {bot_spacer}px;" }
                }
            }
        }
    }
}

/// Sentinel for "insert *after* target": the `apply()` handler treats
/// `before: Some(next_sibling)` as before-next, which is functionally
/// after-target. Computing `next_sibling` here would require
/// re-walking the tree; we leave it `None` (append to parent) for
/// the after-mode, which is good enough for now and matches a
/// common drop-on-bottom-half UX.
#[allow(non_snake_case)]
fn None_after(_target: Uuid) -> Option<Uuid> {
    None
}

/// Where in the target row the pointer landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropMode {
    Before,
    After,
    Child,
}

#[derive(Clone, Copy, Debug)]
pub struct DropEvent {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub target_parent: Option<Uuid>,
    pub mode: DropMode,
}

#[derive(Props, PartialEq, Clone)]
struct GridRowProps {
    task: GanttTask,
    level: u32,
    has_children: bool,
    height: f32,
    selected: bool,
    columns: Vec<GanttColumn>,
    readonly: bool,
    dragging_id: Signal<Option<Uuid>>,
    drop_target: Signal<Option<(Uuid, DropMode)>>,
    on_select: EventHandler<()>,
    on_toggle: EventHandler<()>,
    on_open_editor: EventHandler<()>,
    on_drop: EventHandler<DropEvent>,
}

#[component]
fn GridRow(props: GridRowProps) -> Element {
    let mut dragging_id = props.dragging_id;
    let mut drop_target = props.drop_target;
    let indent_px = props.level as f32 * 16.0;
    let task = &props.task;
    let chevron = if props.has_children {
        if task.open { "▾" } else { "▸" }
    } else {
        ""
    };
    let bg = if props.selected {
        "bg-accent/40"
    } else {
        "hover:bg-accent/20"
    };
    let h = props.height;
    let task_id = task.id;
    let parent_id = task.parent;
    let row_h = props.height;

    // Drop indicator decoration: highlight the top/bottom border or
    // the row background depending on `drop_target`'s mode for this id.
    let dt = *drop_target.read();
    let is_target_self = props
        .dragging_id
        .read()
        .as_ref()
        .is_some_and(|d| *d == task_id);
    let row_opacity = if is_target_self { "0.3" } else { "1" };
    let (top_ind, bot_ind, child_ind) = match dt {
        Some((id, mode)) if id == task_id => match mode {
            DropMode::Before => ("border-t-2 border-t-primary", "", ""),
            DropMode::After => ("", "border-b-2 border-b-primary", ""),
            DropMode::Child => ("", "", "ring-2 ring-primary ring-inset"),
        },
        _ => ("", "", ""),
    };

    rsx! {
        div {
            class: "flex border-b border-border/40 cursor-pointer select-none {bg} {top_ind} {bot_ind} {child_ind}",
            style: "height: {h}px; opacity: {row_opacity};",
            draggable: !props.readonly,
            ondragstart: move |e: Event<DragData>| {
                if props.readonly { return; }
                let dt = e.data().data_transfer();
                let _ = dt.set_data("text/x-gantt-id", &task_id.to_string());
                dragging_id.set(Some(task_id));
            },
            ondragend: move |_| {
                dragging_id.set(None);
                drop_target.set(None);
            },
            ondragover: move |e: Event<DragData>| {
                if props.readonly { return; }
                if props
                    .dragging_id
                    .peek()
                    .as_ref()
                    .is_none_or(|d| *d == task_id)
                {
                    return;
                }
                e.prevent_default();
                // Decide insertion mode from pointer-y within the row.
                let local_y = e.data().element_coordinates().y as f32;
                let mode = if local_y < row_h * 0.25 {
                    DropMode::Before
                } else if local_y > row_h * 0.75 {
                    DropMode::After
                } else {
                    DropMode::Child
                };
                let cur = drop_target.peek().clone();
                if cur != Some((task_id, mode)) {
                    drop_target.set(Some((task_id, mode)));
                }
            },
            ondrop: move |e: Event<DragData>| {
                e.prevent_default();
                let dt = e.data().data_transfer();
                let raw = dt.get_data("text/x-gantt-id").unwrap_or_default();
                let Ok(source_id) = raw.parse::<Uuid>() else { return };
                if source_id == task_id {
                    return;
                }
                let mode = props
                    .drop_target
                    .peek()
                    .and_then(|(id, m)| (id == task_id).then_some(m))
                    .unwrap_or(DropMode::After);
                props.on_drop.call(DropEvent {
                    source_id,
                    target_id: task_id,
                    target_parent: parent_id,
                    mode,
                });
            },
            onclick: move |_| props.on_select.call(()),
            ondoubleclick: move |_| props.on_open_editor.call(()),
            for col in props.columns.iter() {
                {
                    let on_toggle = props.on_toggle.clone();
                    rsx!(
                        div {
                            key: "c-{col.kind:?}",
                            class: "flex-none flex items-center px-2 border-r border-border/40 text-sm",
                            style: "width: {col.width}px;",
                            {column_cell(col, task, props.level, props.has_children, chevron, indent_px, on_toggle)}
                        }
                    )
                }
            }
        }
    }
}

fn column_cell(
    col: &GanttColumn,
    task: &GanttTask,
    level: u32,
    has_children: bool,
    chevron: &'static str,
    indent_px: f32,
    on_toggle: EventHandler<()>,
) -> Element {
    let type_dot = match task.task_type {
        TaskType::Task => "bg-primary",
        TaskType::Summary => "bg-foreground",
        TaskType::Milestone => "bg-accent-foreground",
    };
    match col.kind {
        ColumnKind::Name => rsx! {
            div {
                class: "flex items-center w-full",
                style: "padding-left: {indent_px}px;",
                button {
                    class: "w-5 h-5 flex-none flex items-center justify-center text-muted-foreground",
                    onclick: move |e: Event<MouseData>| {
                        e.stop_propagation();
                        on_toggle.call(());
                    },
                    "{chevron}"
                }
                span { class: "w-2 h-2 rounded-full mr-2 flex-none {type_dot}" }
                span { class: "truncate", "{task.text}" }
                {
                    let _ = level;
                    let _ = has_children;
                    rsx!()
                }
            }
        },
        ColumnKind::Start => {
            let s = task.start.format("%Y-%m-%d").to_string();
            rsx!(span { class: "tabular-nums text-muted-foreground", "{s}" })
        }
        ColumnKind::End => {
            let s = task.end.format("%Y-%m-%d").to_string();
            rsx!(span { class: "tabular-nums text-muted-foreground", "{s}" })
        }
        ColumnKind::Progress => {
            let pct = (task.progress.clamp(0.0, 1.0) * 100.0).round() as i32;
            rsx!(span { class: "tabular-nums", "{pct}%" })
        }
        ColumnKind::Duration => {
            let days = (task.end - task.start).num_days();
            rsx!(span { class: "tabular-nums", "{days}d" })
        }
        ColumnKind::Type => {
            let label = match task.task_type {
                TaskType::Task => "Task",
                TaskType::Summary => "Summary",
                TaskType::Milestone => "Milestone",
            };
            rsx!(span { class: "text-muted-foreground", "{label}" })
        }
    }
}
