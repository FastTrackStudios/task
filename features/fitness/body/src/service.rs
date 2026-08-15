//! The `BodyService` wire surface now lives in the wasm-clean
//! [`fitness_proto`] crate. Re-exported here so the existing
//! `body::service::*` paths (and the `Store` impl in
//! [`crate::store`]) keep working. The architect-emitted vox bits
//! (client / dispatcher / descriptor / serve) are surfaced at the
//! crate root via [`crate`]'s re-exports.

pub use fitness_proto::body::service::{BodyError, BodyService};

#[cfg(feature = "vox")]
pub use fitness_proto::body::service::{
    BodyServiceClient, BodyServiceRpc, BodyServiceRpcDispatcher, Service,
    body_service_rpc_service_descriptor, layer, serve,
};
