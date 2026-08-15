//! Contextual relevance now lives in the wasm-clean
//! [`task_proto`] crate (pure functions over the wire types —
//! the server applies them inside `TaskService::query`, the web
//! UI calls the same functions client-side). Re-exported here so
//! the existing `task::relevance::*` paths keep working.

pub use task_proto::relevance::{
    RelevanceContext, condense_next_per_anchor, condense_next_per_project, filter_relevant,
    is_relevant, next_action_key, partition_triage, relevance_rank,
};
