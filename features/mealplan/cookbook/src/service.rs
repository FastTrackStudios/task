//! The `CookbookService` wire surface now lives in the wasm-clean
//! [`cookbook_proto`] crate. Re-exported here so the existing
//! `cookbook::service::*` paths (and the `Store` impl in
//! [`crate::store`]) keep working. The architect-emitted vox bits
//! (client / dispatcher / descriptor / serve) are surfaced at the
//! crate root via [`crate`]'s re-exports.

pub use cookbook_proto::service::{CookbookError, CookbookService, CookbookServiceRpc};

#[cfg(feature = "vox")]
pub use cookbook_proto::service::{
    CookbookServiceClient, CookbookServiceRpcDispatcher, Service,
    cookbook_service_rpc_service_descriptor, layer, serve,
};
