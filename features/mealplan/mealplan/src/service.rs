//! The `MealplanService` wire surface now lives in the wasm-clean
//! [`mealplan_proto`] crate. Re-exported here so the existing
//! `mealplan::service::*` paths (and the `Store` impl in
//! [`crate::store`]) keep working. The architect-emitted vox bits
//! (client / dispatcher / descriptor / serve) are surfaced at the
//! crate root via [`crate`]'s re-exports.

pub use mealplan_proto::service::{MealplanError, MealplanService, MealplanServiceRpc};

#[cfg(feature = "vox")]
pub use mealplan_proto::service::{
    MealplanServiceClient, MealplanServiceRpcDispatcher, Service, layer,
    mealplan_service_rpc_service_descriptor, serve,
};
