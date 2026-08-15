//! Exercise-catalog wire surface — `Exercise` (+ `Category` /
//! `Equipment` / `Mechanics` / `Force`) + the `ExercisesService`
//! CRUD trait.

pub mod model;
pub mod service;

pub use model::{Category, Equipment, Exercise, Force, Mechanics, StringList};
pub use service::{ExercisesError, ExercisesService, ExercisesServiceRpc};

#[cfg(feature = "vox")]
pub use service::{
    ExercisesServiceClient, ExercisesServiceRpcDispatcher as ExercisesDispatcher,
    Service as ExercisesServiceBridge,
    exercises_service_rpc_service_descriptor as exercises_service_descriptor,
    layer as exercises_service_layer, serve as serve_exercises_service,
};
