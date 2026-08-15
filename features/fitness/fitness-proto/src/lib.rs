// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the fitness suite.
//!
//! This proto owns the **wasm-clean wire surface** for all four
//! fitness sub-domains so the web UI can talk to the services
//! directly:
//!
//! - [`body`] — `BodyMetric` time series + `BodyService`.
//! - [`exercises`] — `Exercise` catalog + `ExercisesService`.
//! - [`workouts`] — `Routine` / `WorkoutSession` + `WorkoutsService`.
//! - [`intake`] — daily `IntakeLog` + `IntakeService` (embeds
//!   `cookbook_proto::Nutrition`).
//!
//! The sibling `body` / `exercises` / `workouts` / `intake`
//! crates sit on top of this proto and own the parse / serialize
//! / scan side plus the disk-backed `Store` backends — exactly
//! like `locations` sits on top of `locations-proto`. The native
//! crates re-export this proto so CLI / server paths keep working.

pub mod body;
pub mod exercises;
pub mod intake;
pub mod workouts;
