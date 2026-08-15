//! `Meal` — one cooked-or-planned meal on the calendar.
//!
//! Meals live as markdown pages with YAML frontmatter
//! (`type: meal`) under `<vault>/Projects/Mealplan/meals/`.
//! They reference recipes by **vault-relative path** (cooklang
//! `.cook` files have no UUID; the path is identity). Once
//! cooked, the meal records which pantry items it consumed.
//!
//! This wire model lives in the wasm-clean `mealplan-proto` crate
//! so the web UI can bind the `MealplanService` client directly.
//! The `Meal::nutrition_total` aggregation (which needs the
//! native `pantry` unit-conversion layer) stays in the native
//! `mealplan` crate as the [`mealplan::MealNutrition`] extension
//! trait.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `Vec<String>` newtype — JSON column. Shared across
/// meal types.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct StringList(pub Vec<String>);

impl StringList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for StringList {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for StringList {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for StringList {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for StringList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// `Vec<PantryDeduction>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct PantryDeductions(pub Vec<PantryDeduction>);

impl PantryDeductions {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<PantryDeduction>> for PantryDeductions {
    fn from(v: Vec<PantryDeduction>) -> Self {
        Self(v)
    }
}

impl FromIterator<PantryDeduction> for PantryDeductions {
    fn from_iter<I: IntoIterator<Item = PantryDeduction>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for PantryDeductions {
    type Target = Vec<PantryDeduction>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "meals", repo)]
pub struct Meal {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Display title — `"Tuesday Dinner"`, `"Post-workout
    /// shake"`, or just the recipe name on simple days.
    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Calendar date this meal is scheduled for (or was
    /// cooked on, once `status` is `cooked`).
    #[serde(rename = "scheduledFor")]
    #[architect(filterable, sortable)]
    pub scheduled_for: NaiveDate,

    /// Free-form slot. Canonical set in [`Slot`].
    #[serde(default = "default_slot")]
    #[architect(filterable)]
    pub slot: String,

    /// Servings to make — multiplier on each referenced
    /// recipe's base `servings`. `1` by default; bump for
    /// batch-cook days.
    #[serde(default = "default_servings")]
    pub servings: u32,

    /// Recipes this meal is built from. Vault-relative paths
    /// to `.cook` files (cooklang convention). Multi-recipe
    /// meals supported.
    #[serde(
        skip_serializing_if = "StringList::is_empty",
        default,
        rename = "recipePaths"
    )]
    #[architect(json)]
    pub recipe_paths: StringList,

    /// Free-form lifecycle status. Canonical set in
    /// [`Status`]: `planned` / `cooked` / `skipped` /
    /// `eating-out`.
    #[serde(default = "default_status")]
    #[architect(filterable)]
    pub status: String,

    /// What got pulled from the pantry when this meal was
    /// cooked. Populated by `MealplanService::cook`;
    /// empty while the meal is still `planned`.
    #[serde(
        skip_serializing_if = "PantryDeductions::is_empty",
        default,
        rename = "pantryDeductions"
    )]
    #[architect(json)]
    pub pantry_deductions: PantryDeductions,

    /// Free-form tags — `"meal-prep"`, `"date-night"`,
    /// `"leftovers"`.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub tags: StringList,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateCreated"
    )]
    pub date_created: Option<DateTime<Utc>>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateModified"
    )]
    pub date_modified: Option<DateTime<Utc>>,

    /// Markdown body — pre-cook notes ("thaw chicken Tuesday
    /// morning"), post-cook review ("too much salt"),
    /// photos.
    #[serde(skip)]
    pub details: String,
}

/// One row in [`Meal::pantry_deductions`] — what got
/// consumed when the meal was cooked.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct PantryDeduction {
    /// `pantry::PantryItem` id.
    #[serde(rename = "itemId")]
    pub item_id: Uuid,

    pub qty: f64,

    #[serde(default)]
    pub unit: String,
}

fn default_servings() -> u32 {
    1
}

fn default_slot() -> String {
    Slot::Dinner.as_str().to_string()
}

fn default_status() -> String {
    Status::Planned.as_str().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Breakfast,
    Lunch,
    Dinner,
    Snack,
}

impl Slot {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Breakfast => "breakfast",
            Self::Lunch => "lunch",
            Self::Dinner => "dinner",
            Self::Snack => "snack",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "breakfast" | "brunch" => Some(Self::Breakfast),
            "lunch" => Some(Self::Lunch),
            "dinner" | "supper" => Some(Self::Dinner),
            "snack" => Some(Self::Snack),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Planned,
    Cooked,
    Skipped,
    EatingOut,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Cooked => "cooked",
            Self::Skipped => "skipped",
            Self::EatingOut => "eating-out",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "planned" | "scheduled" => Some(Self::Planned),
            "cooked" | "done" | "made" => Some(Self::Cooked),
            "skipped" | "cancelled" => Some(Self::Skipped),
            "eating-out" | "eating_out" | "out" | "restaurant" => Some(Self::EatingOut),
            _ => None,
        }
    }
}
