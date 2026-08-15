//! The `TaskInfo` model now lives in the wasm-clean
//! [`task_proto`] crate. Re-exported here so the existing
//! `task::model::*` paths (parser / writer / scanner / backend
//! and every downstream consumer) keep working.

pub use task_proto::model::{
    AgentRefList, Estimate, Priority, Relation, RelationKind, RelationList, Status, StringList,
    TaskInfo, TimeEntries, TimeEntry, UuidList, WorkflowAttrs, close_open_time_entries,
    is_due_on_or_before, status_is_open, status_is_terminal, track_status_transition,
};
