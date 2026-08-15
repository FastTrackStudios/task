//! The per-project state registry now lives in the wasm-clean
//! [`project_proto`] crate. Re-exported here so the existing
//! `project::states::*` paths keep working.

pub use project_proto::states::{
    StateDef, StateGroup, StatesConfig, default_states, resolve_state_group,
};
