//! The `ProjectService` wire surface now lives in the wasm-clean
//! [`project_proto`] crate. Re-exported here so the existing
//! `project::service::*` paths (and the
//! [`crate::backend::ProjectBackend`] impl) keep working. The
//! architect-emitted vox bits (client / dispatcher / descriptor /
//! serve) are surfaced at the crate root via [`crate`]'s
//! re-exports.

pub use project_proto::service::{ProjectError, ProjectEvent, ProjectService, ProjectServiceRpc};

#[cfg(feature = "vox")]
pub use project_proto::service::{
    ProjectServiceClient, ProjectServiceRpcDispatcher, Service, layer,
    project_service_rpc_service_descriptor, serve,
};

#[cfg(feature = "vox")]
pub use project_proto::service::{
    ProjectServiceStream, ProjectServiceStreamClient, ProjectServiceStreamSource,
    project_service_stream_service_descriptor, stream_layer, stream_serve,
};
