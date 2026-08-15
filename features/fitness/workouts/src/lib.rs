// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `workouts` — programmed templates (`Routine`) +
//! per-session logs (`WorkoutSession` / `LoggedSet`) in a
//! `vault::Vault`. Two page types under one crate because
//! sessions reference routines and logs reference
//! exercises; splitting them across crates adds wiring
//! with no benefit.
//!
//! Routine model is *optional* — a `WorkoutSession` with
//! `routine_id: None` is a perfectly valid ad-hoc workout.
//! Captures both programmed and free-style lifting.
//!
//! Surface:
//! - [`Routine`] / [`RoutineDay`] / [`RoutineSlot`] — the
//!   template hierarchy.
//! - [`WorkoutSession`] / [`LoggedSet`] / [`SessionStatus`]
//!   — the performance log.
//! - [`parse_routine`] / [`parse_session`] /
//!   [`looks_like_routine`] / [`looks_like_session`].
//! - [`serialize_routine`] / [`serialize_session`] +
//!   `write_*` helpers.
//! - [`scan_routines`] / [`scan_sessions`] /
//!   [`sessions_between`] / [`sessions_for_exercise`].
//! - [`WorkoutsService`] — `#[architect::rpc]` trait,
//!   single surface for both routines and sessions.
//! - [`Store`] — file-backed impl.

#![cfg(not(target_arch = "wasm32"))]

pub mod entity;
pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod write;

pub use entity::{Routines, Sessions};
pub use model::{LoggedSet, Routine, RoutineDay, RoutineSlot, SessionStatus, WorkoutSession};
pub use parse::{ParseError, looks_like_routine, looks_like_session, parse_routine, parse_session};
pub use scan::{scan_routines, scan_sessions, sessions_between, sessions_for_exercise};
pub use service::{WorkoutsError, WorkoutsService};

#[cfg(feature = "vox")]
pub use fitness_proto::workouts::{
    WorkoutsDispatcher, WorkoutsServiceBridge, WorkoutsServiceClient, serve_workouts_service,
    workouts_service_descriptor, workouts_service_layer,
};
#[cfg(feature = "vox")]
pub use service::{WorkoutsServiceRpc, WorkoutsServiceRpcDispatcher};
pub use store::Store;
pub use write::{
    WriteError, default_routine_path, default_session_path, serialize_routine, serialize_session,
    write_routine, write_session,
};
