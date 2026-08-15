//! The `Item` model now lives in the wasm-clean
//! [`inventory_proto`] crate so the web UI can bind to the wire
//! contract directly. Re-exported here so the existing
//! `inventory::model::*` paths keep working.

pub use inventory_proto::model::{Condition, Item, Status, StringList};
