//! Task bars — render + drag/resize/progress/link initiation.
//!
//! All pointer-move/up handling lives at the gantt-root, so this
//! file only needs to *initiate* drags (`onpointerdown` on bar / edge
//! / progress handles) and convert clicks on link dots into the
//! pending-link state.
//!
//! Linking flow (svar-shaped):
//! 1. Click an anchor on bar A → `linking = Some({A, side})`.
//! 2. Other bars' anchors light up.
//! 3. Click an anchor on bar B → fires `AddLink`, clears state.
//! 4. Esc cancels (handled at gantt-root).
//!
//! Selection is multi: plain click replaces, ctrl/meta toggles,
//! shift extends (the gantt-root currently treats shift as toggle —
//! true range needs an anchor signal, see follow-up).

use dioxus::prelude::*;
use architect_ui::prelude::*;

use crate::scales::ScaleGrid;
use crate::store::{GanttEvent, LaidOutTask, SelectMode, is_descendant};
use crate::types::{LinkType, TaskType};

use super::context_menu::{ContextMenuTarget, use_context_menu};
use super::drag::{DragKind, DragState, LinkPending, use_drag_context};
use super::gantt::GanttContext;

#[derive(Props, PartialEq, Clone)]
pub struct BarsProps {
    pub rows: Vec<LaidOutTask>,
    pub grid: ScaleGrid,
}

#[component]
pub fn Bars(props: BarsProps) -> Element {
    rsx! {
        for r in props.rows.iter().cloned() {
            Bar { key: "{r.task.id}", row: r }
        }
    }
}

#[derive(Props, PartialEq, Clone)]
struct BarProps {
    row: LaidOutTask,
}

#[component]
fn Bar(props: BarProps) -> Element {
    let ctx = use_context::<GanttContext>();
    let on_event = ctx.on_event.clone();
    let drag = use_drag_context();
    let mut drag_state = drag.state;
    let mut linking = drag.linking;
    // Local "editing the label" state — toggled on dblclick.
    let mut editing_label = use_signal(|| false);

    let r = props.row.clone();
    let id = r.task.id;
    let readonly = ctx.state.read().readonly;
    let selected = ctx.state.read().selected.contains(&id);

    // Live offsets while dragging. Three follow rules:
    // 1. This bar is the drag target → offset / width-delta / progress
    //    per drag kind.
    // 2. Drag target is a Summary and this bar is a descendant → cascade.
    // 3. This bar is in the multi-selection and the drag is a Move on
    //    another selected bar → cascade dx so the whole set drags.
    let active = drag.state.read().clone();
    let mut progress_override: Option<f32> = None;
    let (offset_x, width_delta_start, width_delta_end) = match active.as_ref() {
        Some(d) if d.committed && d.task_id == id => match d.kind {
            DragKind::Move => (d.dx, 0.0, 0.0),
            DragKind::ResizeStart => (d.dx, -d.dx, 0.0),
            DragKind::ResizeEnd => (0.0, 0.0, d.dx),
            DragKind::Progress => {
                let pct = (d.orig_progress + d.dx / d.orig_w).clamp(0.0, 1.0);
                progress_override = Some(pct);
                (0.0, 0.0, 0.0)
            }
        },
        Some(d) if d.committed && d.kind == DragKind::Move => {
            let s = ctx.state.read();
            let parent_is_summary = s
                .tasks
                .iter()
                .find(|t| t.id == d.task_id)
                .is_some_and(|t| matches!(t.task_type, TaskType::Summary));
            let in_multi =
                s.selected.len() > 1 && s.selected.contains(&d.task_id) && s.selected.contains(&id);
            if (parent_is_summary && is_descendant(&s, d.task_id, id)) || in_multi {
                (d.dx, 0.0, 0.0)
            } else {
                (0.0, 0.0, 0.0)
            }
        }
        _ => (0.0, 0.0, 0.0),
    };
    let x = r.x + offset_x;
    let w = (r.w + width_delta_start + width_delta_end).max(2.0);
    let y = r.y;
    let h = r.h;

    let progress = progress_override.unwrap_or(r.task.progress).clamp(0.0, 1.0);
    let progress_w = w * progress;
    let is_milestone = matches!(r.task.task_type, TaskType::Milestone);
    let is_summary = matches!(r.task.task_type, TaskType::Summary);
    let custom_color = r.task.color.clone();

    let pending = *linking.read();
    let is_link_source = pending.is_some_and(|p| p.source == id);
    let link_pending_any = pending.is_some();

    // Visual choice for the bar. No `absolute` here — the outer
    // wrapper div owns positioning so the architect-ui Tooltip wrapper
    // sits at the same coords as the bar (see comment in milestone
    // branch above).
    let bar_class = if is_milestone {
        "flex items-center justify-center"
    } else if is_summary {
        "rounded-sm bg-foreground text-background shadow-sm"
    } else if custom_color.is_some() {
        if selected {
            "rounded-md ring-2 ring-primary text-white shadow"
        } else {
            "rounded-md text-white shadow-sm transition-shadow hover:shadow-md"
        }
    } else if selected {
        "rounded-md bg-primary/90 ring-2 ring-primary text-primary-foreground shadow"
    } else {
        "rounded-md bg-primary text-primary-foreground shadow-sm transition-colors hover:bg-primary/90"
    };
    let bar_style_color = custom_color
        .as_ref()
        .map(|c| format!("background-color: {c};"))
        .unwrap_or_default();
    let bar_cursor = if readonly { "default" } else { "grab" };
    let bar_opacity = if readonly { "opacity-95" } else { "" };

    // Tooltip lines.
    let start_label = r.task.start.format("%b %d, %Y").to_string();
    let end_label = r.task.end.format("%b %d, %Y").to_string();
    let pct_label = (progress * 100.0).round() as i32;

    // ---- Drag starters. Closures are Copy thanks to all captures
    //      being Copy (Signal, Uuid, DateTime, f32 …). ----
    let r_for_drag = r.clone();
    let bar_w = r.w;
    let bar_progress = r.task.progress;
    let mut start_drag = move |kind: DragKind, page_x: f32| {
        if readonly {
            return;
        }
        drag_state.set(Some(DragState {
            kind,
            task_id: id,
            start_x: page_x,
            dx: 0.0,
            orig_start: r_for_drag.task.start,
            orig_end: r_for_drag.task.end,
            orig_w: bar_w,
            orig_progress: bar_progress,
            committed: false,
        }));
    };

    // ---- Link-dot click. Two-stage: arm source / fire AddLink. ----
    let mut on_link_click = move |from_start: bool| {
        if readonly {
            return;
        }
        let mut linking_w = linking.write();
        match *linking_w {
            None => {
                *linking_w = Some(LinkPending {
                    source: id,
                    from_start,
                });
            }
            Some(p) if p.source == id => {
                // Cancel on second click of the same bar.
                *linking_w = None;
            }
            Some(p) => {
                let link_type = match (p.from_start, from_start) {
                    (false, true) => LinkType::E2s,
                    (false, false) => LinkType::E2e,
                    (true, true) => LinkType::S2s,
                    (true, false) => LinkType::S2e,
                };
                *linking_w = None;
                drop(linking_w);
                on_event.call(GanttEvent::AddLink {
                    source: p.source,
                    target: id,
                    link_type,
                });
            }
        }
    };
    let on_link_click_start = move |e: Event<MouseData>| {
        e.stop_propagation();
        on_link_click(true);
    };
    let on_link_click_end = move |e: Event<MouseData>| {
        e.stop_propagation();
        on_link_click(false);
    };

    // ---- Plain bar click: select (with modifier-aware mode). ----
    let ctx_for_click = ctx.clone();
    let on_bar_click = move |e: Event<MouseData>| {
        e.stop_propagation();
        // Suppress click if a drag just committed.
        if drag.state.read().as_ref().is_some_and(|s| s.committed) {
            return;
        }
        let mods = e.data().modifiers();
        let mode = if mods.ctrl() || mods.meta() {
            SelectMode::Toggle
        } else if mods.shift() {
            SelectMode::Range
        } else {
            SelectMode::Replace
        };
        ctx_for_click
            .on_event
            .call(GanttEvent::Select { id: Some(id), mode });
    };
    // Double-click → inline rename. Milestones / summaries skip this
    // (no label to edit). Enter or blur commits via UpdateText.
    let on_bar_dbl = move |_e: Event<MouseData>| {
        if readonly || is_milestone {
            return;
        }
        editing_label.set(true);
    };

    // Right-click → context menu anchored at the pointer.
    let mut menu = use_context_menu().open;
    let on_context = move |e: Event<MouseData>| {
        e.prevent_default();
        // Some browsers send contextmenu BEFORE pointerup, leaving
        // drag_state still seeded by the right-button pointerdown.
        // Clear it so the menu's `committed`-suppression path
        // doesn't snap any bar position when the user picks an item.
        drag_state.set(None);
        let p = e.data().client_coordinates();
        menu.set(Some(ContextMenuTarget {
            x: p.x as f32,
            y: p.y as f32,
            task_id: id,
        }));
    };

    // Dot visibility / source-highlight rules.
    let dot_visible_cls = if link_pending_any {
        "opacity-100"
    } else {
        "opacity-0 group-hover:opacity-100"
    };
    let dot_source_cls = if is_link_source {
        "border-[3px] border-primary bg-primary/30 shadow-[0_0_0_3px_var(--tw-ring-color)] ring-primary/40"
    } else {
        "border-2 border-primary bg-card"
    };

    // Anchors are reused for both milestones and ordinary bars; the
    // milestone branch builds its own DOM so it needs them too.
    let left_anchor = rsx! {
        div {
            class: "absolute top-0 bottom-0 flex items-center justify-center cursor-crosshair z-10 group",
            style: "left: -18px; width: 14px;",
            onclick: on_link_click_start,
            onpointerdown: move |e: Event<PointerData>| e.stop_propagation(),
            title: "Click to start a link from this bar's start.",
            div { class: "w-3.5 h-3.5 rounded-full shadow-sm transition-all {dot_source_cls} {dot_visible_cls}" }
        }
    };
    let right_anchor = rsx! {
        div {
            class: "absolute top-0 bottom-0 flex items-center justify-center cursor-crosshair z-10 group",
            style: "right: -18px; width: 14px;",
            onclick: on_link_click_end,
            onpointerdown: move |e: Event<PointerData>| e.stop_propagation(),
            title: "Click to start a link from this bar's end.",
            div { class: "w-3.5 h-3.5 rounded-full shadow-sm transition-all {dot_source_cls} {dot_visible_cls}" }
        }
    };

    // ---- Milestone branch. ----
    //
    // Note on layout: the bar / diamond is `position: absolute`. The
    // architect-ui `Tooltip` positions its content via CSS classes anchored
    // to the Tooltip wrapper's box — and that wrapper is normal-flow
    // (`relative inline-flex`). If we let the bar sit at the natural
    // flow origin and rely on its own `absolute` to put it visually
    // at (x, y), the Tooltip wrapper still thinks it's at (0, 0) of
    // the chart body and the popup lands clipped in the top-left.
    //
    // Fix: outer `absolute` wrapper holds the (x, y, w, h) box.
    // Inside, the Tooltip fills that box (`block w-full h-full`),
    // so its computed position matches the bar's. The bar visual
    // itself fills the Tooltip via `inset-0`.
    if is_milestone {
        let size = h.min(28.0);
        let cx = x + w / 2.0 - size / 2.0;
        let cy = y + h / 2.0 - size / 2.0;
        let milestone_color = custom_color
            .clone()
            .unwrap_or_else(|| "#f59e0b".to_string());
        return rsx! {
            div {
                class: "absolute",
                style: "left: {cx}px; top: {cy}px; width: {size}px; height: {size}px;",
                Tooltip { class: "block w-full h-full",
                    TooltipTrigger { class: "block w-full h-full",
                        div {
                            class: "relative w-full h-full group",
                            onclick: on_bar_click,
                            ondoubleclick: on_bar_dbl,
                            oncontextmenu: on_context,
                            onpointerdown: move |e| {
                                let px = e.data().page_coordinates().x as f32;
                                start_drag(DragKind::Move, px);
                            },
                            div {
                                class: "w-full h-full rotate-45 shadow-md transition-transform group-hover:scale-110",
                                style: "background: {milestone_color};",
                            }
                            {left_anchor}
                            {right_anchor}
                        }
                    }
                    TooltipContent {
                        div { class: "text-xs",
                            div { class: "font-medium", "{r.task.text}" }
                            div { class: "text-muted-foreground", "Milestone · {start_label}" }
                        }
                    }
                }
            }
        };
    }

    // ---- Ordinary / summary bar branch. ----
    let bar_dx_start_drag = start_drag;
    rsx! {
        div {
            class: "absolute",
            style: "left: {x}px; top: {y}px; width: {w}px; height: {h}px;",
            Tooltip { class: "block w-full h-full",
            TooltipTrigger { class: "block w-full h-full",
                div {
                    class: "absolute inset-0 {bar_class} group {bar_opacity}",
                    style: "touch-action: none; cursor: {bar_cursor}; {bar_style_color}",
                    onclick: on_bar_click,
                    ondoubleclick: on_bar_dbl,
                    oncontextmenu: on_context,
                    onpointerdown: move |e| {
                        let px = e.data().page_coordinates().x as f32;
                        bar_dx_start_drag.clone()(DragKind::Move, px);
                    },

                    // Progress fill.
                    if !is_summary && progress > 0.0 {
                        div {
                            class: "absolute left-0 top-0 bottom-0 rounded-l-md pointer-events-none",
                            style: "width: {progress_w}px; background: rgba(0,0,0,0.25);",
                        }
                    }

                    // Label — span at rest, contenteditable input
                    // when `editing_label` is true.
                    if *editing_label.read() {
                        input {
                            class: "absolute inset-0 px-2 text-xs font-medium bg-transparent text-inherit outline-none ring-1 ring-primary/60 rounded-md",
                            value: "{r.task.text}",
                            autofocus: true,
                            onpointerdown: move |e: Event<PointerData>| e.stop_propagation(),
                            onclick: move |e: Event<MouseData>| e.stop_propagation(),
                            onkeydown: move |e: Event<KeyboardData>| {
                                let k = e.data().key();
                                if k == Key::Enter || k == Key::Escape {
                                    // Browser will fire `onchange` on
                                    // blur after we close, so the
                                    // committed value flows through.
                                    editing_label.set(false);
                                    e.prevent_default();
                                }
                                // Stop the gantt-root's hotkey handler
                                // from interpreting typed letters as
                                // shortcuts while editing.
                                e.stop_propagation();
                            },
                            onchange: {
                                let on_event = on_event.clone();
                                move |e: FormEvent| {
                                    on_event.call(GanttEvent::UpdateText {
                                        id,
                                        text: e.value(),
                                    });
                                    editing_label.set(false);
                                }
                            },
                            onblur: move |_| editing_label.set(false),
                        }
                    } else {
                        span {
                            class: "absolute inset-0 flex items-center px-2 text-xs font-medium truncate pointer-events-none",
                            "{r.task.text}"
                        }
                    }

                    {left_anchor}
                    {right_anchor}

                    // Progress drag handle — a thin marker at the
                    // progress%-position; drag horizontally to change
                    // the value. Leaf bars only.
                    if !is_summary && !readonly {
                        {
                            let mut start_drag2 = bar_dx_start_drag.clone();
                            rsx!(
                                div {
                                    class: "absolute top-0 bottom-0 w-1 cursor-ew-resize bg-white/70 shadow z-30 hover:w-1.5 hover:bg-white",
                                    style: "left: {progress_w}px; transform: translateX(-50%);",
                                    onpointerdown: move |e| {
                                        e.stop_propagation();
                                        let px = e.data().page_coordinates().x as f32;
                                        start_drag2(DragKind::Progress, px);
                                    },
                                    title: "Drag to change progress ({pct_label}%)",
                                }
                            )
                        }
                    }

                    // Resize handles — leaf bars only, in read-write mode.
                    if !is_summary && !readonly {
                        {
                            let mut start_drag_l = bar_dx_start_drag.clone();
                            let mut start_drag_r = bar_dx_start_drag.clone();
                            rsx!(
                                div {
                                    class: "absolute left-0 top-0 bottom-0 w-2 cursor-ew-resize bg-transparent hover:bg-white/30 z-20",
                                    onpointerdown: move |e| {
                                        e.stop_propagation();
                                        let px = e.data().page_coordinates().x as f32;
                                        start_drag_l(DragKind::ResizeStart, px);
                                    },
                                }
                                div {
                                    class: "absolute right-0 top-0 bottom-0 w-2 cursor-ew-resize bg-transparent hover:bg-white/30 z-20",
                                    onpointerdown: move |e| {
                                        e.stop_propagation();
                                        let px = e.data().page_coordinates().x as f32;
                                        start_drag_r(DragKind::ResizeEnd, px);
                                    },
                                }
                            )
                        }
                    }
                }
            }
            TooltipContent {
                div { class: "text-xs",
                    div { class: "font-medium", "{r.task.text}" }
                    div { class: "text-muted-foreground", "{start_label} → {end_label} · {pct_label}%" }
                }
            }
            }
        }
    }
}
