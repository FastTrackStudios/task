//! The `BodyMetric` / `BodyEntry` / `MetricKind` models now live
//! in the wasm-clean [`fitness_proto`] crate so the web UI can
//! bind to the wire contract directly. Re-exported here so the
//! existing `body::model::*` paths keep working.

pub use fitness_proto::body::model::*;
