//! Mutation vocabulary emitted by the views. The owning
//! consumer decides whether to write through to a vault, CRDT,
//! or anything else — the UI is dumb.

use uuid::Uuid;

use crate::TaskInfo;

#[derive(Clone, Debug, PartialEq)]
pub enum TaskMutation {
    /// Add a fresh task. Carries the full payload (consumer
    /// chose the id at parse time).
    Create {
        task: TaskInfo,
    },
    /// Whole-record replace (e.g. detail panel save).
    Update {
        task: TaskInfo,
    },
    /// Quick path: just flip the status. Saves a clone on
    /// every checkbox toggle.
    SetStatus {
        id: Uuid,
        status: String,
    },
    /// Quick path: just flip the priority.
    SetPriority {
        id: Uuid,
        priority: String,
    },
    Delete {
        id: Uuid,
    },
}
