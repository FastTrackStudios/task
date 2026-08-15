//! Body-metric wire surface — `BodyMetric` / `BodyEntry` /
//! `MetricKind` + the `BodyService` CRUD trait.

pub mod model;
pub mod service;

pub use model::{BodyEntry, BodyMetric, Entries, MetricKind, Tags};
pub use service::{BodyError, BodyService, BodyServiceRpc};

#[cfg(feature = "vox")]
pub use service::{
    BodyServiceClient, BodyServiceRpcDispatcher as BodyDispatcher, Service as BodyServiceBridge,
    body_service_rpc_service_descriptor as body_service_descriptor, layer as body_service_layer,
    serve as serve_body_service,
};
