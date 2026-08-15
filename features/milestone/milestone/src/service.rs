//! The `MilestoneService` wire surface now lives in the
//! wasm-clean [`milestone_proto`] crate. Re-exported here so the
//! existing `milestone::service::*` paths (and the
//! [`crate::backend::MilestoneBackend`] impl) keep working. The
//! architect-emitted vox bits (client / dispatcher / descriptor /
//! serve) are surfaced at the crate root via [`crate`]'s
//! re-exports.

pub use milestone_proto::service::{MilestoneError, MilestoneService};

#[cfg(feature = "vox")]
pub use milestone_proto::service::{
    MilestoneServiceClient, MilestoneServiceRpc, MilestoneServiceRpcDispatcher, Service, layer,
    milestone_service_rpc_service_descriptor, serve,
};
