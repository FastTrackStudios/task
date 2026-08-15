//! `IntakeService` — CRUD over daily logs + the typed
//! `log_recipe` / `log_pantry` / `log_freeform` helpers
//! that resolve nutrition from mealplan and append.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::model::{IntakeEntry, IntakeLog};
use cookbook_proto::Nutrition;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum IntakeError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("mealplan: {0}")]
    Mealplan(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait IntakeService {
    fn list(&self) -> Result<Vec<IntakeLog>, IntakeError>;

    fn get(&self, id: &str) -> Result<IntakeLog, IntakeError>;

    /// Get-or-create the log page for `date`. Used as the
    /// upsert primitive by the `log_*` helpers so the
    /// caller never has to think about whether today's
    /// page exists yet.
    fn for_day(&self, date: &str) -> Result<IntakeLog, IntakeError>;

    fn create(&self, log: IntakeLog) -> Result<IntakeLog, IntakeError>;

    fn update(&self, log: IntakeLog) -> Result<IntakeLog, IntakeError>;

    fn delete(&self, id: &str) -> Result<(), IntakeError>;

    /// Log `servings` servings of `recipe_id` against
    /// `date`. Resolves per-serving nutrition from the
    /// recipe, scales by servings, appends an
    /// `IntakeEntry { source: Recipe, ... }`.
    fn log_recipe(
        &self,
        date: &str,
        recipe_id: &str,
        servings: f64,
        slot: &str,
    ) -> Result<IntakeLog, IntakeError>;

    /// Log `qty` of pantry item `item_id` against `date`.
    /// Resolves per-unit nutrition from the pantry item's
    /// `nutrition_per_unit` + `nutrition_unit`, scales,
    /// appends.
    fn log_pantry(
        &self,
        date: &str,
        item_id: &str,
        qty: f64,
        slot: &str,
    ) -> Result<IntakeLog, IntakeError>;

    /// Log a hand-entered row (restaurant food, friend's
    /// cooking, label estimates). Nutrition is carried on
    /// the entry — no source lookup.
    fn log_freeform(
        &self,
        date: &str,
        name: &str,
        nutrition: Nutrition,
        slot: &str,
    ) -> Result<IntakeLog, IntakeError>;

    /// Append `entry` directly. Use when the caller has
    /// already resolved nutrition (e.g. via barcode scan
    /// + portion math); the `log_*` helpers wrap this.
    fn log_entry(&self, date: &str, entry: IntakeEntry) -> Result<IntakeLog, IntakeError>;
}
