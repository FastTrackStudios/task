//! In-memory store + reducer. Demo wiring uses this directly;
//! a vault-backed consumer ignores it and routes the mutation
//! to disk.

use uuid::Uuid;

use crate::{TaskInfo, TaskMutation};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskState {
    pub tasks: Vec<TaskInfo>,
}

impl TaskState {
    #[must_use]
    pub fn find(&self, id: Uuid) -> Option<&TaskInfo> {
        self.tasks.iter().find(|t| t.id == id)
    }
}

/// In-place reducer matching the mutation vocabulary. Invalid
/// ids are silently dropped — UI views show whatever the next
/// render of `state.tasks` carries.
pub fn apply(state: &mut TaskState, mu: &TaskMutation) {
    match mu {
        TaskMutation::Create { task } => state.tasks.push(task.clone()),
        TaskMutation::Update { task } => {
            if let Some(slot) = state.tasks.iter_mut().find(|t| t.id == task.id) {
                *slot = task.clone();
                slot.date_modified = Some(chrono::Utc::now());
            }
        }
        TaskMutation::SetStatus { id, status } => {
            if let Some(t) = state.tasks.iter_mut().find(|t| t.id == *id) {
                t.status = status.clone();
                t.date_modified = Some(chrono::Utc::now());
                // Stamp `completedDate` whenever the status
                // classifies into the `completed` state group
                // (group-routed, so custom names like `shipped`
                // count) — list grouping by completion day works
                // without re-reading the file mtime.
                if project_proto::resolve_state_group(None, status) == project_proto::StateGroup::Completed {
                    t.completed_date = Some(chrono::Local::now().date_naive());
                } else {
                    t.completed_date = None;
                }
            }
        }
        TaskMutation::SetPriority { id, priority } => {
            if let Some(t) = state.tasks.iter_mut().find(|t| t.id == *id) {
                t.priority = priority.clone();
                t.date_modified = Some(chrono::Utc::now());
            }
        }
        TaskMutation::Delete { id } => {
            state.tasks.retain(|t| t.id != *id);
        }
    }
}
