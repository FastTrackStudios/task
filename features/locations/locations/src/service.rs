//! The `LocationsService` wire surface now lives in the
//! wasm-clean [`locations_proto`] crate. Re-exported here so the
//! existing `locations::service::*` paths (and the `Store` impl
//! in [`crate::store`]) keep working. The architect-emitted vox
//! bits (client / dispatcher / descriptor / serve) are surfaced
//! at the crate root via [`crate`]'s re-exports.

pub use locations_proto::service::{LocationsError, LocationsService};

#[cfg(feature = "vox")]
pub use locations_proto::service::{
    LocationsServiceClient, LocationsServiceRpc, LocationsServiceRpcDispatcher, Service, layer,
    locations_service_rpc_service_descriptor, serve,
};
