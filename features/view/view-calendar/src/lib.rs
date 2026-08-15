//! Calendar — Dioxus port of the Google-Calendar surface (month /
//! week / day views), inspired by Untitled UI's calendar set and
//! Nextcloud's FullCalendar.io wrapping.
//!
//! Dumb UI crate: feed events in via props, receive mutations back
//! via `EventHandler<CalendarMutation>`. No CRDT/DB awareness. Use
//! architect-ui primitives only.
//!
//! # Out of scope for v1
//!
//! Recurrence (RRULE), multiple calendars / layers, timezones,
//! year view, mini-calendar sidebar, agenda view, attendees,
//! reminders, overlap column-splitting. The data shape leaves room
//! for these (event already carries `start: DateTime<Utc>` etc.)
//! but the UI doesn't render them yet.
//!
//! # Layout
//!
//! - **Month**: 6×7 day grid, events as horizontal chips inside each
//!   day cell. Drag-move emits `Reschedule` with the start clamped
//!   to the dropped day; resize is not exposed here.
//! - **Week**: 7-column time grid with a 24-hour axis on the left.
//!   Events absolute-positioned. Drag-move slides start, drag the
//!   bottom edge to resize.
//! - **Day**: same as week but with `days: Vec<NaiveDate>` of length
//!   1 — shared `TimeGridView` component.

pub mod components;
pub mod cycle_overlay;
pub mod layout;
pub mod recurrence;
pub mod store;
pub mod time;
pub mod types;

pub use components::{Calendar, CalendarProps, SlotGrid, SlotGridProps, SlotItem, SlotRow};
pub use store::{CalendarMutation, CalendarState, apply};
pub use types::{BlockEdit, CalendarEvent, ColorTag, EventId, TemplateBlock, ViewMode};
