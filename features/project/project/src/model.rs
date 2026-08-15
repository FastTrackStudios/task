//! The `ProjectInfo` model now lives in the wasm-clean
//! [`project_proto`] crate. Re-exported here so the existing
//! `project::model::*` paths (parser / writer / scanner /
//! backend) keep working.

pub use project_proto::model::{ProjectInfo, Status, Tags};
