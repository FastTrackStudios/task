//! Dioxus components — the parts a consumer renders.
//!
//! Only [`Gantt`] is public; everything else is internal plumbing
//! threaded together by the root component.

mod bars;
mod chart;
mod context_menu;
mod drag;
mod editor;
mod gantt;
mod grid;
mod links;
mod timescale;
mod toolbar;

pub use gantt::{Gantt, GanttProps};
