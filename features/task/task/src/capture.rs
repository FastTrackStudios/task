//! Natural-language capture now lives in the wasm-clean
//! [`task_proto`] crate (it is a pure `&str` → `TaskInfo`
//! builder, and the browser quick-add drives it too).
//! Re-exported here so the existing `task::capture::*` paths keep
//! working.

pub use task_proto::capture::{capture, infer_project_id};
