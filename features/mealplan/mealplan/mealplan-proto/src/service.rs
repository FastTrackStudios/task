//! `MealplanService` — wire surface that ties recipes,
//! pantry stock, and the calendar together.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::fulfillment::{CookReceipt, Fulfillment};
use crate::model::{Meal, PantryDeduction};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum MealplanError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("pantry: {0}")]
    Pantry(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait MealplanService {
    fn list(&self) -> Result<Vec<Meal>, MealplanError>;

    fn get(&self, id: &str) -> Result<Meal, MealplanError>;

    fn create(&self, meal: Meal) -> Result<Meal, MealplanError>;

    fn update(&self, meal: Meal) -> Result<Meal, MealplanError>;

    fn rename(&self, id: &str, new_path: &str) -> Result<Meal, MealplanError>;

    fn delete(&self, id: &str) -> Result<(), MealplanError>;

    /// Mark the meal as `cooked`, stamp today's deductions
    /// onto its `pantry_deductions`, and consume each row
    /// from the pantry. Atomic at the meal-row level only —
    /// pantry consumes are sequential, so on partial failure
    /// some stock may already be debited. (Future:
    /// transactional consume across rows.)
    fn cook(&self, id: &str, deductions: Vec<PantryDeduction>) -> Result<Meal, MealplanError>;

    /// Mark a meal as `skipped` without touching the pantry.
    fn skip(&self, id: &str) -> Result<Meal, MealplanError>;

    /// Mark a meal as `eating-out` — planned, not cooked, and eaten
    /// anyway. Like [`MealplanService::skip`] it leaves the pantry
    /// alone, and the distinction is the point: a skipped meal is one
    /// that didn't happen, and looking back at a week, "we ate out on
    /// Tuesday" and "Tuesday fell through" are different facts.
    fn eat_out(&self, id: &str) -> Result<Meal, MealplanError>;

    /// "Can I make this recipe right now from what's in the
    /// pantry?" Pulls the recipe + current pantry snapshot
    /// and runs the native fulfillment check. Returns the
    /// fulfillment result including per-ingredient
    /// shortages.
    fn can_cook(&self, recipe_path: &str, servings: u32) -> Result<Fulfillment, MealplanError>;

    /// Cook a recipe directly (no planned meal): compute the pantry
    /// deductions for `servings` and consume them from stock. Returns
    /// a [`CookReceipt`] — the per-ingredient deductions actually
    /// applied (matched, convertible, capped at what's on hand) plus
    /// the ingredients that were skipped and why — so the caller can
    /// show what was used and what wasn't.
    fn cook_recipe(&self, recipe_path: &str, servings: u32) -> Result<CookReceipt, MealplanError>;
}
