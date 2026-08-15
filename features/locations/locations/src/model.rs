//! The `Location` model now lives in the wasm-clean
//! [`locations_proto`] crate so the web UI can bind to the wire
//! contract directly. Re-exported here so the existing
//! `locations::model::*` paths keep working.

pub use locations_proto::model::{Kind, Location, Tags};
