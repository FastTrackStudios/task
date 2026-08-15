//! Drag + link-pending state shared between bars and chart.
//!
//! Lives at the [`super::gantt::Gantt`] root and provided to children
//! via context. The root installs gantt-wide `onpointermove` /
//! `onpointerup` handlers (so events keep firing even when the
//! pointer drifts outside the chart pane, mirroring svar's window-
//! level `mouseup`).

use chrono::{DateTime, Utc};
use dioxus::prelude::*;

use crate::types::TaskId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragKind {
    Move,
    ResizeStart,
    ResizeEnd,
    /// Dragging the progress-marker on the bar. `dx` updates the
    /// progress percentage from `orig_progress`.
    Progress,
}

#[derive(Clone, Debug)]
pub struct DragState {
    pub kind: DragKind,
    pub task_id: TaskId,
    pub start_x: f32,
    pub dx: f32,
    pub orig_start: DateTime<Utc>,
    pub orig_end: DateTime<Utc>,
    /// Width of the bar at drag start, in px. Used by the progress
    /// drag to translate `dx` into a progress delta (`dx / orig_w`).
    pub orig_w: f32,
    /// Progress at drag start, in 0..1.
    pub orig_progress: f32,
    /// True once the drag exceeds the move-threshold; until then it's
    /// treated as a click. Matches svar's `taskMove.start` flag.
    pub committed: bool,
}

/// Half-formed link: source bar + anchor side (`start` or `end`).
/// Set on a single click; consumed when the user clicks any other
/// bar's link dot.
#[derive(Clone, Copy, Debug)]
pub struct LinkPending {
    pub source: TaskId,
    pub from_start: bool,
}

#[derive(Clone, Copy)]
pub struct DragContext {
    pub state: Signal<Option<DragState>>,
    pub linking: Signal<Option<LinkPending>>,
}

pub fn use_drag_context() -> DragContext {
    use_context::<DragContext>()
}
