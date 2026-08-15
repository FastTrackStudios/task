//! The workout models — `Routine` / `RoutineDay` /
//! `RoutineSlot` (template) and `WorkoutSession` / `LoggedSet` /
//! `SessionStatus` (performance log) — now live in the
//! wasm-clean [`fitness_proto`] crate so the web UI can bind to
//! the wire contract directly. Re-exported here so the existing
//! `workouts::model::*` paths keep working.

pub use fitness_proto::workouts::model::*;
