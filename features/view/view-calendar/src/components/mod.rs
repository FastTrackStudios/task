//! Calendar component tree.
//!
//! Root is [`calendar::Calendar`]. Children consume drag state via
//! [`drag::DragContext`] and dispatch mutations back through the
//! `on_event` prop wired by [`calendar::Calendar`].

mod all_day_strip;
mod calendar;
mod day_view;
mod drag;
mod event_chip;
mod event_editor;
mod month_view;
mod now_line;
mod slot_grid;
mod style;
mod time_grid;
mod toolbar;
mod week_view;

pub use calendar::{Calendar, CalendarProps};
pub use slot_grid::{SlotGrid, SlotGridProps, SlotItem, SlotRow};
