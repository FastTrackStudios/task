// Dioxus 0.7's `Callback<T>` is `Copy`, but this port hits
// the upstream's `Callback.clone()` pattern in ~40 places
// across `components/`. Reverse-engineering each Svelte
// reactive-statement → Dioxus signal flow to remove the
// clones (while preserving the port's structure) buys very
// little against the diff churn. Lint stays off in this
// crate only; everything else in the workspace enforces it.
#![allow(clippy::clone_on_copy)]

//! Gantt chart — Dioxus port of svar-widgets/gantt.
//!
//! Dumb UI crate: feed `tasks` and `links` in via props, receive
//! mutations back via `EventHandler<GanttEvent>`. No CRDT/DB
//! awareness. Use architect-ui primitives only (Sheet, Button, Input, etc).
//!
//! # Scope of this port
//!
//! The svar reference is ~10k LOC of Svelte. This port covers the
//! substantial core:
//!
//! - Types: tasks (task/summary/milestone), links (FS/SS/FF/SF), scales, zoom
//! - Timeline with multi-row scale header + today line + weekend shading
//! - Hierarchical task list (sidebar grid) with expand/collapse
//! - Bars with drag-to-move, edge-resize, progress fill
//! - Summary rollup bars
//! - Dependency arrows (SVG) + create-link drag from bar edges
//! - Side editor (architect-ui Sheet) for inline task editing
//! - Zoom levels (day/week/month/quarter/year)
//! - Markers (custom vertical lines)
//!
//! Out of scope (follow-ups):
//! virtualized scroll, undo/redo, critical path, baselines, auto-
//! scheduling, calendars/holidays, MS Project import, PDF/PNG export,
//! split tasks, slack visualization, context menu, hotkeys, toolbar.

pub mod components;
pub mod scales;
pub mod store;
pub mod time;
pub mod types;

pub use components::{Gantt, GanttProps};
pub use store::{GanttEvent, GanttState};
pub use types::{
    ColumnKind, DurationUnit, GanttColumn, GanttLink, GanttTask, LengthUnit, LinkType, Marker,
    ScaleConfig, TaskId, TaskType, ZoomConfig, ZoomLevel, default_columns,
};
