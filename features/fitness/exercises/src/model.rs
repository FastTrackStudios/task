//! The `Exercise` model (+ `Category` / `Equipment` /
//! `Mechanics` / `Force`) now lives in the wasm-clean
//! [`fitness_proto`] crate so the web UI can bind to the wire
//! contract directly. Re-exported here so the existing
//! `exercises::model::*` paths keep working.

pub use fitness_proto::exercises::model::*;
