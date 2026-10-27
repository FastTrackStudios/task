//! Top-level view tree.

mod composer;
mod detail;
mod detail_full;
pub use composer::{
    CaptureKind, Composer, ComposerProps, Trigger, compose, drop_last_word, kind_from_text,
    project_title, trigger_at_end,
};
mod issue_view;
pub use issue_view::{
    IssueView, IssueViewProps, assignee_of, labels_of, toggle_label, with_assignee,
};
mod kanban;
mod links;
mod menu;
pub use menu::ChoiceMenu;
mod list;
pub use list::{TaskSection, classify as task_section};
mod palette;
mod picker;
pub use picker::{PickOption, PropertyPicker, filtered as picker_filtered, match_rank};
mod properties;
pub use properties::{
    AssigneeChip, CHIP_CLASS, CHIP_QUIET_CLASS, Hotkey, LabelsChip, PRIORITIES, PersonDot,
    PriorityChip, PriorityIcon, ProjectChip, PropertyRow, STATUSES, StatusChip, StatusIcon,
    canonical_priority, canonical_status, label_color,
};
mod quick_add;
mod row;
pub use row::{AnchorChip, CheckboxButton, clock_running, days_late, late_label, owner_name};
mod session_history;
mod subtasks;
mod tasks_app;
mod time;
mod triage;
pub use triage::{TriageStrip, TriageStripProps};
mod workflow;

pub use detail_full::{
    ClaimState, LinkedTaskRef, SubtaskRow, TaskDetailFull, TaskDetailFullProps, resolve_links,
    short_id,
};
pub use links::{LinkChips, LinkChipsProps};
pub use quick_add::{QuickAdd, QuickAddProps};
pub use session_history::{
    SessionEvent, SessionHistory, SessionHistoryProps, activity_label, merge_session_events,
    payload_preview,
};
pub use subtasks::{SubtasksBoard, SubtasksBoardProps, subtask_summary};
pub use tasks_app::{TasksApp, TasksAppProps, ViewMode};
pub use time::{
    TimeSection, TimeSectionProps, format_minutes, recurrence_summary, sum_logged_minutes,
};
pub use workflow::{WorkflowSection, WorkflowSectionProps, estimate_label};
