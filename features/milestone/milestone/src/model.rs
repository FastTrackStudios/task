//! The `Milestone` model now lives in the wasm-clean
//! [`milestone_proto`] crate. Re-exported here so the existing
//! `milestone::model::*` paths (parser / writer / backend) keep
//! working.

pub use milestone_proto::model::{Milestone, Status, Tags};
