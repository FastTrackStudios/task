//! The task-graph relation rules now live in the wasm-clean
//! [`task_proto`] crate (pure functions over `&[TaskInfo]`, no
//! fs — the client runs them too). Re-exported here so the
//! existing `task::relations::*` paths keep working.

pub use task_proto::relations::{
    ReverseRelation, arrange_families, blockers_of, blocking, cascade_status, click_transition,
    edges, outgoing, reverse_index, reverse_relations_for,
};
