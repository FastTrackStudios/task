//! Recipe-fulfillment **wire types** — `Fulfillment` /
//! `Shortage` / `SubstitutionSuggestion` and their enums.
//!
//! These are the values [`crate::service::MealplanService::can_cook`]
//! returns, so they live in this wasm-clean proto. The pure
//! `check` / `check_with_subs` / `check_nested` matching logic
//! (which needs the native `pantry` unit-conversion layer) stays
//! in the native `mealplan::fulfillment` module and re-exports
//! these types.

use facet::Facet;
use pantry_proto::SubReason;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct Fulfillment {
    pub can_cook: bool,
    /// Ingredients already satisfied from current stock, scaled to
    /// the requested servings. The flip side of `missing` — together
    /// they partition the recipe's non-recipe-ref ingredients, so a
    /// frontend can render "have / need" without re-deriving it.
    #[serde(default)]
    pub have: Vec<HaveLine>,
    pub missing: Vec<Shortage>,
}

/// One satisfied ingredient in [`Fulfillment::have`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct HaveLine {
    /// Recipe ingredient name (cooklang `@name`).
    pub name: String,
    /// Amount the recipe calls for at the requested servings, if it
    /// gives a quantity (`None` for "to taste" ingredients).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub need: Option<f64>,
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct Shortage {
    /// Recipe ingredient name (cooklang `@name`).
    pub name: String,

    #[serde(rename = "ingredientIdx")]
    pub ingredient_idx: u32,

    pub need: f64,
    pub have: f64,
    pub unit: String,
    pub reason: ShortageReason,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suggestions: Vec<SubstitutionSuggestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct SubstitutionSuggestion {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "toItemId")]
    pub to_item_id: Option<uuid::Uuid>,
    pub ratio: f64,
    pub need: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub have: Option<f64>,
    #[serde(default)]
    pub reasons: Vec<SubReason>,
    pub source: SubstitutionSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum SubstitutionSource {
    PantryItem,
    Registry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum ShortageReason {
    NotInPantry,
    InsufficientQty,
    UnitMismatch,
    OptionalNoQty,
}

impl ShortageReason {
    /// Human label for the shortage — the single source of truth so
    /// every frontend (CLI, UI) reads the same words.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotInPantry => "not in pantry",
            Self::InsufficientQty => "not enough in stock",
            Self::UnitMismatch => "unit mismatch",
            Self::OptionalNoQty => "optional, no amount given",
        }
    }
}

/// The outcome of cooking a recipe straight from the pantry —
/// what [`crate::service::MealplanService::cook_recipe`] actually
/// debited from stock, and what it had to leave alone. The cook
/// gets a per-ingredient receipt instead of a bare count, and a
/// list of what wasn't deducted (with the reason) so nothing is
/// silently dropped.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Facet)]
pub struct CookReceipt {
    /// Ingredients debited from stock, in pantry-item units.
    pub deducted: Vec<DeductionLine>,
    /// Ingredients the deduction couldn't touch, with why.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub skipped: Vec<SkippedIngredient>,
}

/// One applied deduction, named for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct DeductionLine {
    /// `pantry::PantryItem` id the amount came off.
    #[serde(rename = "itemId")]
    pub item_id: uuid::Uuid,

    /// Recipe ingredient name (cooklang `@name`).
    pub ingredient: String,

    /// Amount consumed, in `unit`.
    pub qty: f64,

    /// The pantry item's unit the deduction is denominated in.
    #[serde(default)]
    pub unit: String,
}

/// An ingredient `cook_recipe` left untouched + the reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct SkippedIngredient {
    /// Recipe ingredient name (cooklang `@name`).
    pub ingredient: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum SkipReason {
    /// The recipe gives no quantity (e.g. `@salt` to taste), so
    /// there's nothing to deduct.
    NoQuantity,
    /// No pantry item matched the ingredient name.
    NoPantryMatch,
    /// Matched a pantry item, but the units don't convert (e.g.
    /// recipe "cloves" vs pantry "grams").
    InconvertibleUnit,
    /// Matched + convertible, but the pantry is empty for it.
    OutOfStock,
}
