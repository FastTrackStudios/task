//! Dependency arrow layer.
//!
//! The path strings cached on [`LaidOutLink`] reflect *static* bar
//! positions. While the user is mid-drag, this layer recomputes each
//! affected link's elbow path so the arrows follow the bars in real
//! time. Two follow rules:
//!
//! - Endpoint whose bar IS the drag target → shift by `dx`.
//! - Endpoint whose bar is a descendant of a Summary being moved
//!   → shift by `dx` (matches the cascade preview in `bars.rs`).
//!
//! Anything else uses the cached path verbatim.

use dioxus::prelude::*;

use crate::store::{GanttEvent, LaidOutLink, is_descendant, link_path};
use crate::types::TaskType;

use super::drag::{DragKind, use_drag_context};
use super::gantt::GanttContext;

#[derive(Props, PartialEq, Clone)]
pub struct LinkLayerProps {
    pub links: Vec<LaidOutLink>,
    pub width: f32,
    pub height: f32,
}

#[component]
pub fn LinkLayer(props: LinkLayerProps) -> Element {
    let ctx = use_context::<GanttContext>();
    let on_event = ctx.on_event.clone();
    let drag = use_drag_context();
    let w = props.width;
    let h = props.height;

    // Pre-compute the per-endpoint dx offsets driven by the active
    // drag, if any. Done once per render — cheaper than recomputing
    // inside every link loop iteration.
    let active = drag.state.read().clone();
    let active_dx = active
        .as_ref()
        .filter(|d| d.committed && d.kind == DragKind::Move)
        .map(|d| d.dx);
    let active_id = active.as_ref().filter(|d| d.committed).map(|d| d.task_id);
    let summary_active = if let (Some(dx), Some(id)) = (active_dx, active_id) {
        let s = ctx.state.read();
        let is_summary = s
            .tasks
            .iter()
            .find(|t| t.id == id)
            .is_some_and(|t| matches!(t.task_type, TaskType::Summary));
        if is_summary { Some((id, dx)) } else { None }
    } else {
        None
    };

    let offset_for = move |task_id| -> f32 {
        if let Some(active_d) = active.as_ref() {
            if active_d.committed && active_d.task_id == task_id {
                // Direct drag target — shift by full dx for moves;
                // resize-start shifts the start endpoint only, but
                // resize doesn't apply to summaries so we don't need
                // to disambiguate further: the link endpoint at
                // `task_id`'s anchor side just rides the bar edge it
                // touches. For simplicity treat any non-Move drag as
                // 0 here — the bar edges still re-render, just the
                // path stays static during resize.
                if active_d.kind == DragKind::Move {
                    return active_d.dx;
                }
            }
        }
        if let Some((summary_id, dx)) = summary_active {
            let s = ctx.state.read();
            if is_descendant(&s, summary_id, task_id) {
                return dx;
            }
        }
        0.0
    };

    rsx! {
        svg {
            class: "absolute inset-0 pointer-events-none",
            width: "{w}",
            height: "{h}",
            xmlns: "http://www.w3.org/2000/svg",
            defs {
                marker {
                    id: "gantt-arrow",
                    "viewBox": "0 0 10 10",
                    "refX": "10",
                    "refY": "5",
                    "markerWidth": "6",
                    "markerHeight": "6",
                    "orient": "auto-start-reverse",
                    path {
                        d: "M 0 0 L 10 5 L 0 10 z",
                        fill: "currentColor",
                    }
                }
            }
            for l in props.links.iter().cloned() {
                {
                    let id = l.link.id;
                    let on_event = on_event.clone();
                    let s_off = offset_for(l.source_id);
                    let t_off = offset_for(l.target_id);
                    // Recompute path only if either endpoint moved.
                    let d = if s_off == 0.0 && t_off == 0.0 {
                        l.path.clone()
                    } else {
                        link_path(l.sx + s_off, l.sy, l.tx + t_off, l.ty, l.link.link_type)
                    };
                    rsx!(
                        path {
                            key: "{id}",
                            d: "{d}",
                            stroke: "currentColor",
                            stroke_width: "1.5",
                            fill: "none",
                            class: "text-muted-foreground hover:text-destructive cursor-pointer pointer-events-auto",
                            marker_end: "url(#gantt-arrow)",
                            onclick: move |_| on_event.call(GanttEvent::DeleteLink { id }),
                        }
                    )
                }
            }
        }
    }
}
