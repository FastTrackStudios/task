//! `scheduling-ui` — Dioxus components for the scheduling feature.
//!
//! v1 surface: a personal day-template viewer / editor. Cal.com-
//! style event-type editor + public booking page land in follow-up
//! commits; the proto + facade are already shaped to receive them.

pub mod components;

pub use components::day_template_view::{DayTemplateView, DayTemplateViewProps};
