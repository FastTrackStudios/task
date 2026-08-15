//! The `InventoryService` wire surface now lives in the
//! wasm-clean [`inventory_proto`] crate. Re-exported here so the
//! existing `inventory::service::*` paths (and the `Store` impl
//! in [`crate::store`]) keep working. The architect-emitted vox
//! bits (client / dispatcher / descriptor / serve) are surfaced
//! at the crate root via [`crate`]'s re-exports.

pub use inventory_proto::service::{InventoryError, InventoryService};

#[cfg(feature = "vox")]
pub use inventory_proto::service::{
    InventoryServiceClient, InventoryServiceRpc, InventoryServiceRpcDispatcher, Service,
    inventory_service_rpc_service_descriptor, layer, serve,
};
