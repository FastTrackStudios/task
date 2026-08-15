//! The `Goal` model now lives in the wasm-clean
//! [`goal_proto`] crate. Re-exported here so the existing
//! `goal::model::*` paths (parser / writer / backend) keep
//! working.

pub use goal_proto::model::{Goal, Kind, Status, Tags};
