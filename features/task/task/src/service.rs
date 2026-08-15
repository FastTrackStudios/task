//! The `TaskService` wire surface now lives in the wasm-clean
//! [`task_proto`] crate. Re-exported here so the existing
//! `task::service::*` paths (and the
//! [`crate::backend::TaskBackend`] impl) keep working. The
//! architect-emitted vox bits (client / dispatcher / descriptor /
//! serve, plus the `#[subscribe] events` stream sibling) are
//! surfaced at the crate root via [`crate`]'s re-exports.

pub use task_proto::service::{
    ClaimResult, TaskError, TaskEvent, TaskListFilter, TaskReverseRelations, TaskService,
    TaskServiceRpc,
};

#[cfg(feature = "vox")]
pub use task_proto::service::{
    Service, TaskServiceClient, TaskServiceRpcDispatcher, layer, serve,
    task_service_rpc_service_descriptor,
};

// The `#[subscribe] fn events` stream sibling.
#[cfg(feature = "vox")]
pub use task_proto::service::{
    TaskServiceStream, TaskServiceStreamClient, TaskServiceStreamSource, stream_layer, stream_serve,
    task_service_stream_service_descriptor,
};
