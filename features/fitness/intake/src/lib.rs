// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope to keep migration uniform with other
// architect-derived features.
#![allow(unexpected_cfgs)]

//! `intake` — daily food intake log. One page per day
//! (`type: intake-log`, default path
//! `intake/<YYYY-MM-DD>.md`) listing what you ate.
//!
//! Each [`IntakeEntry`] references a `cookbook::Recipe`,
//! a `pantry::PantryItem`, or carries free-form
//! nutrition. The `log_*` service helpers resolve
//! nutrition from `mealplan` (recipe per-serving, pantry
//! per-100g) and stamp it onto the entry — so a deleted
//! or renamed source doesn't lose your calorie data.
//!
//! Daily totals are a pure function of the entries (see
//! [`IntakeLog::total`]); the fitness facade combines
//! those with workout volume + body metrics for the
//! "how's my training going" surface.

#![cfg(not(target_arch = "wasm32"))]

pub mod entity;
pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod write;

pub use entity::IntakeLogs;
pub use model::{IntakeEntry, IntakeLog, IntakeSource, scale_nutrition};
pub use parse::{ParseError, looks_like_intake, parse_page};
pub use scan::{between, for_day, scan_vault};
pub use service::{IntakeError, IntakeService};

#[cfg(feature = "vox")]
pub use fitness_proto::intake::{
    IntakeDispatcher, IntakeServiceBridge, IntakeServiceClient, intake_service_descriptor,
    intake_service_layer, serve_intake_service,
};
#[cfg(feature = "vox")]
pub use service::{IntakeServiceRpc, IntakeServiceRpcDispatcher};
pub use store::Store;
pub use write::{WriteError, default_intake_path, serialize_intake, write_intake};
