//! The `PantryService` wire surface now lives in the wasm-clean
//! [`pantry_proto`] crate. Re-exported here so the existing
//! `pantry::service::*` paths (and the `Store` impl in
//! [`crate::store`]) keep working. The architect-emitted vox bits
//! (client / dispatcher / descriptor / serve) are surfaced at the
//! crate root via [`crate`]'s re-exports.

pub use pantry_proto::service::{
    BarcodeResolution, ConsumeReceipt, EntryDebit, PantryError, PantryService, PantryServiceRpc,
};

#[cfg(feature = "vox")]
pub use pantry_proto::service::{
    PantryServiceClient, PantryServiceRpcDispatcher, Service, layer,
    pantry_service_rpc_service_descriptor, serve,
};
