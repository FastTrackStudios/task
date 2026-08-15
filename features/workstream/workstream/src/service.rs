//! The `WorkstreamService` wire surface lives in the wasm-clean
//! [`workstream_proto`] crate. Re-exported here so
//! `workstream::service::*` paths (and the
//! [`crate::backend::WorkstreamBackend`] impl) match the sibling
//! feature crates. The architect-emitted vox bits (client /
//! dispatcher / descriptor / serve / stream layer) are surfaced
//! at the crate root via [`crate`]'s re-exports.

pub use workstream_proto::service::{
    StateGroupCounts, WorkstreamError, WorkstreamEvent, WorkstreamRollup, WorkstreamService,
    WorkstreamWithRollup,
};

#[cfg(feature = "vox")]
pub use workstream_proto::service::{
    Service, WorkstreamServiceClient, WorkstreamServiceRpc, WorkstreamServiceRpcDispatcher,
    WorkstreamServiceStream, WorkstreamServiceStreamClient, WorkstreamServiceStreamSource, layer,
    serve, stream_layer, stream_serve, workstream_service_rpc_service_descriptor,
    workstream_service_stream_service_descriptor,
};
