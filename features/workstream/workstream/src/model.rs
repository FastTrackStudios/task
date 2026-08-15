//! The `Workstream` model lives in the wasm-clean
//! [`workstream_proto`] crate. Re-exported here so the
//! `workstream::model::*` paths (parser / writer / backend)
//! match the sibling feature crates.

pub use workstream_proto::model::{AgentRefList, Links, Status, Workstream};
