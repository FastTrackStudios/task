//! The `IntakeService` wire surface now lives in the wasm-clean
//! [`fitness_proto`] crate. Re-exported here so the existing
//! `intake::service::*` paths (and the `Store` impl in
//! [`crate::store`]) keep working.

pub use fitness_proto::intake::service::{IntakeError, IntakeService};

#[cfg(feature = "vox")]
pub use fitness_proto::intake::service::{
    IntakeServiceClient, IntakeServiceRpc, IntakeServiceRpcDispatcher, Service,
    intake_service_rpc_service_descriptor, layer, serve,
};
