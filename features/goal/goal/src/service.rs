//! The `GoalService` wire surface now lives in the wasm-clean
//! [`goal_proto`] crate. Re-exported here so the existing
//! `goal::service::*` paths (and the [`crate::backend::GoalBackend`]
//! impl) keep working. The architect-emitted vox bits (client /
//! dispatcher / descriptor / serve / stream host) are surfaced at the
//! crate root via [`crate`]'s re-exports.

pub use goal_proto::service::{GoalError, GoalEvent, GoalService, GoalServiceRpc};

#[cfg(feature = "vox")]
pub use goal_proto::service::{
    GoalServiceClient, GoalServiceRpcDispatcher, GoalServiceStream, GoalServiceStreamClient,
    GoalServiceStreamSource, Service, goal_service_rpc_service_descriptor,
    goal_service_stream_service_descriptor, layer, serve, stream_layer, stream_serve,
};
