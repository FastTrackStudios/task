//! Tasks UI — TaskNotes-inspired list + kanban over a flat
//! `Vec<TaskInfo>`.
//!
//! The components render the authoritative wire model
//! (`task_proto::TaskInfo`) directly — there is no UI-side mirror
//! of the persistence shape. A consumer hands its rows straight
//! in and handles [`TaskMutation`] callbacks; this crate stays
//! storage-agnostic (no fs, no RPC).
//!
//! The presentation half — duration formatting, the human labels
//! and board order for [`Status`] / [`Priority`], and the derived
//! reads the board wants ([`TaskDisplay`]) — lives in [`display`].

//!
//! # Hosting these components outside Task (Blitz included)
//!
//! Everything here renders under Blitz (dioxus-native: Stylo, Taffy,
//! Vello) as well as in a browser. A host outside this repo mounts four
//! sheets, in this order, as static strings — Blitz loads no external
//! stylesheet — and gives the components its colours through
//! architect-ui's token custom properties on any ancestor:
//!
//! ```rust,ignore
//! rsx! {
//!     document::Style { {architect_ui::THEME_CSS} }     // the tokens
//!     document::Style { {architect_ui::UTILITIES_CSS} } // preflight + its components
//!     document::Style { {task_ui::UTILITIES_CSS} }      // this crate's classes
//!     div {
//!         // The theming hook: any of architect-ui's tokens, scoped here.
//!         style: "--background:#0e0e11;--foreground:#e4e4e7;--border:#222228;--primary:#2563eb;",
//!         TasksApp { tasks, on_event }
//!     }
//! }
//! ```
//!
//! [`Styles`] mounts the three sheets as plain `<style>` elements, which
//! also works in a headless document that has no `<head>`.

pub mod display;
pub mod markdown;
pub mod mutation;
pub mod store;
pub mod views;

/// The compiled Tailwind utilities this crate's components use (theme
/// variables + utilities, no preflight). See the crate docs for the
/// sheets a host mounts with it. Rebuild with `just ui-sheets`.
pub const UTILITIES_CSS: &str = include_str!("../assets/utilities.css");

/// The sheets these components need — architect-ui's tokens and
/// utilities, then this crate's — as `<style>` elements. Mount it once,
/// anywhere above the components; a host that already carries
/// architect-ui's sheets mounts [`UTILITIES_CSS`] alone.
#[dioxus::prelude::component]
pub fn Styles() -> dioxus::prelude::Element {
    use dioxus::prelude::*;
    rsx! {
        style { {architect_ui::THEME_CSS} }
        style { {architect_ui::UTILITIES_CSS} }
        style { {UTILITIES_CSS} }
    }
}

pub use display::{
    BOARD_ORDER, PriorityLabel, StatusLabel, TaskDisplay, clock_label, duration_label,
};
pub use markdown::{Markdown, MarkdownProps, parse_markdown};
pub use mutation::TaskMutation;
pub use store::{TaskState, apply};
// The wire model the components render — re-exported so consumers
// keep the familiar `task_ui::{TaskInfo, Status, …}` paths.
pub use task_proto::{Priority, Status, TaskInfo, TimeEntry};
pub use views::{
    AnchorChip, CheckboxButton, ClaimState, LinkChips, LinkedTaskRef, QuickAdd, SessionEvent,
    SessionHistory, SubtaskRow, SubtasksBoard, TaskDetailFull, TaskDetailFullProps, TaskSection,
    TasksApp, TasksAppProps, TimeSection, TriageStrip, ViewMode, WorkflowSection, clock_running,
    days_late, late_label, owner_name, task_section,
};
// The property system: one typeable picker, five chips, their icons.
pub use views::{
    AssigneeChip, CHIP_CLASS, CHIP_QUIET_CLASS, Hotkey, IssueView, IssueViewProps, LabelsChip,
    PRIORITIES, PersonDot, PickOption, PriorityChip, PriorityIcon, ProjectChip, PropertyPicker,
    PropertyRow, STATUSES, StatusChip, StatusIcon, assignee_of, canonical_priority,
    canonical_status, label_color, labels_of, match_rank, picker_filtered, toggle_label,
    with_assignee,
};
// Making an issue: Linear's composer with Task's natural-language title.
pub use views::{CaptureKind, Composer, ComposerProps, compose, kind_from_text, project_title};
