// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope to keep migration uniform with other
// architect-derived features.
#![allow(unexpected_cfgs)]

//! `exercises` — typed view of exercise-catalog pages in
//! a `vault::Vault`.
//!
//! Exercises are markdown files with YAML frontmatter
//! (`type: exercise`) living under `<vault>/Wiki/Exercises/`
//! so they ride on the wiki feature for curation +
//! wikilinks (`[[scapular retraction]]`,
//! `[[pectoralis-major]]`). Modeled on wger's tables —
//! `Exercise`, `Category`, `Muscle`, `Equipment`,
//! `ExerciseAlias` — flattened onto one page per exercise.
//!
//! Surface:
//! - [`Exercise`] / [`Category`] / [`Equipment`] /
//!   [`Mechanics`] / [`Force`]
//! - [`parse_page`] / [`looks_like_exercise`]
//! - [`serialize_exercise`] / [`write_exercise`]
//! - [`scan_vault`] / [`by_category`] / [`by_equipment`]
//! - [`ExercisesService`] — `#[architect::rpc]` trait
//! - [`Store`] — `Arc<Mutex<vault::Vault>>`-backed impl

#![cfg(not(target_arch = "wasm32"))]

pub mod entity;
pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod write;

pub use entity::Exercises;
pub use model::{Category, Equipment, Exercise, Force, Mechanics};
pub use parse::{ParseError, looks_like_exercise, parse_page};
pub use scan::{by_category, by_equipment, scan_vault};
pub use service::{ExercisesError, ExercisesService};

#[cfg(feature = "vox")]
pub use fitness_proto::exercises::{
    ExercisesDispatcher, ExercisesServiceBridge, ExercisesServiceClient,
    exercises_service_descriptor, exercises_service_layer, serve_exercises_service,
};
#[cfg(feature = "vox")]
pub use service::{ExercisesServiceRpc, ExercisesServiceRpcDispatcher};
pub use store::Store;
pub use write::{WriteError, default_exercise_path, serialize_exercise, write_exercise};
