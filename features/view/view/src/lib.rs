//! `view` — facade for the view feature family.
//!
//! Project-management chart crates (gantt, kanban, calendar, table,
//! …) live under `features/view/view-*`. Every consumer outside
//! `features/view/` should depend on this crate. Internal view
//! crates depend on each other directly and skip the facade.
//!
//! Feature flags (every flag is default-on):
//! - `gantt` — `view-gantt`, exposed as `view::gantt`.
//! - `kanban` — `view-kanban`, exposed as `view::kanban`.
//! - `calendar` — `view-calendar`, exposed as `view::calendar`.
//! - `table` — `view-table` data grid, exposed as `view::table`.
//! - `heatmap` — `view-heatmap` (grid + bars), exposed as
//!   `view::heatmap`.

#[cfg(feature = "gantt")]
pub use view_gantt as gantt;

#[cfg(feature = "kanban")]
pub use view_kanban as kanban;

#[cfg(feature = "calendar")]
pub use view_calendar as calendar;

#[cfg(feature = "table")]
pub use view_table as table;

#[cfg(feature = "heatmap")]
pub use view_heatmap as heatmap;
