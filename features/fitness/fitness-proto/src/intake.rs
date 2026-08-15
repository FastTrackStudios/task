//! Daily food-intake wire surface — `IntakeLog` / `IntakeEntry`
//! / `IntakeSource` (embeds `cookbook_proto::Nutrition`) + the
//! `IntakeService` trait.

pub mod model;
pub mod service;

pub use model::{
    DailyTarget, Entries, IntakeEntry, IntakeLog, IntakeSource, Tags, scale_nutrition,
};
pub use service::{IntakeError, IntakeService, IntakeServiceRpc};

#[cfg(feature = "vox")]
pub use service::{
    IntakeServiceClient, IntakeServiceRpcDispatcher as IntakeDispatcher,
    Service as IntakeServiceBridge,
    intake_service_rpc_service_descriptor as intake_service_descriptor,
    layer as intake_service_layer, serve as serve_intake_service,
};
