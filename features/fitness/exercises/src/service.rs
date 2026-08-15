//! The `ExercisesService` wire surface now lives in the
//! wasm-clean [`fitness_proto`] crate. Re-exported here so the
//! existing `exercises::service::*` paths (and the `Store` impl in
//! [`crate::store`]) keep working.

pub use fitness_proto::exercises::service::{ExercisesError, ExercisesService};

#[cfg(feature = "vox")]
pub use fitness_proto::exercises::service::{
    ExercisesServiceClient, ExercisesServiceRpc, ExercisesServiceRpcDispatcher, Service,
    exercises_service_rpc_service_descriptor, layer, serve,
};
