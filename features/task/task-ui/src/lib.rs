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

pub mod display;
pub mod markdown;
pub mod mutation;
pub mod store;
pub mod views;

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
    AssigneeChip, Hotkey, IssueView, IssueViewProps, LabelsChip, PRIORITIES, PersonDot, PickOption,
    PriorityChip, PriorityIcon, ProjectChip, PropertyPicker, STATUSES, StatusChip, StatusIcon,
    assignee_of, canonical_priority, canonical_status, label_color, labels_of, match_rank,
    picker_filtered, toggle_label, with_assignee,
};
// Making an issue: Linear's composer with Task's natural-language title.
pub use views::{Composer, ComposerProps, compose};
