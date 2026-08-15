//! `fitness` — feature facade tying the four fitness
//! sub-crates together.
//!
//! Vault-superset pattern, same as `mealplan`:
//!
//! ```text
//! features/fitness/
//! ├── exercises/   Exercise catalog (Wiki/Exercises/)
//! ├── workouts/    Routine + WorkoutSession + LoggedSet
//! ├── body/        BodyMetric (weight, bodyfat, ...) time series
//! ├── intake/      Daily food intake log (depends on mealplan)
//! └── fitness/     this crate — facade + DailySummary
//! ```
//!
//! Consumer crates (apps, CLI, agent integrations) should
//! depend on `fitness` alone — the four sub-crates are
//! re-exported so a single import surface stays small.
//! Daily-summary composition lives here so it can pull
//! across surfaces without anyone owning all four.
//!
//! See `~/Development/research/wger/` for the Python
//! reference; this crate's models are direct ports of
//! wger's `Exercise` / `Routine` / `WorkoutSession` /
//! `Measurement` / nutrition tables, flattened onto
//! markdown pages.

#![cfg(not(target_arch = "wasm32"))]

pub mod store;
pub mod summary;

pub use store::Store;
pub use summary::{DailySummary, WeightSnapshot, compute_daily_summary};

// Re-exports — consumers depend on this crate alone.
pub use body;
pub use exercises;
pub use intake;
pub use workouts;
