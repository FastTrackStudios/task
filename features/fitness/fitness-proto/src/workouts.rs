//! Workout wire surface — `Routine` / `RoutineDay` /
//! `RoutineSlot` (template) + `WorkoutSession` / `LoggedSet` /
//! `SessionStatus` (performance log) + the `WorkoutsService`
//! trait covering both.

pub mod model;
pub mod service;

pub use model::{
    LoggedSet, LoggedSets, Routine, RoutineDay, RoutineDays, RoutineSlot, SessionStatus, Tags,
    WorkoutSession,
};
pub use service::{WorkoutsError, WorkoutsService, WorkoutsServiceRpc};

#[cfg(feature = "vox")]
pub use service::{
    Service as WorkoutsServiceBridge, WorkoutsServiceClient,
    WorkoutsServiceRpcDispatcher as WorkoutsDispatcher, layer as workouts_service_layer,
    serve as serve_workouts_service,
    workouts_service_rpc_service_descriptor as workouts_service_descriptor,
};
