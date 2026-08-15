//! The `WorkoutsService` wire surface now lives in the wasm-clean
//! [`fitness_proto`] crate. Re-exported here so the existing
//! `workouts::service::*` paths (and the `Store` impl in
//! [`crate::store`]) keep working.

pub use fitness_proto::workouts::service::{WorkoutsError, WorkoutsService};

#[cfg(feature = "vox")]
pub use fitness_proto::workouts::service::{
    Service, WorkoutsServiceClient, WorkoutsServiceRpc, WorkoutsServiceRpcDispatcher, layer, serve,
    workouts_service_rpc_service_descriptor,
};
