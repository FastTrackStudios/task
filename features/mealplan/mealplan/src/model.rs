//! The `Meal` calendar model (+ `PantryDeduction` / `Slot` /
//! `Status` + the JSON-column newtypes) now lives in the
//! wasm-clean [`mealplan_proto`] crate so the web UI can bind to
//! the wire contract directly. Re-exported here so the existing
//! `mealplan::model::*` paths keep working.
//!
//! The `nutrition_total` aggregation stays in this native crate —
//! it needs the `pantry` unit-conversion layer — as the
//! [`MealNutrition`] extension trait on [`Meal`].

pub use mealplan_proto::model::{
    Meal, PantryDeduction, PantryDeductions, Slot, Status, StringList,
};

/// Native nutrition aggregation for a [`Meal`]. Kept off the
/// wasm-clean proto because it depends on the native `pantry`
/// unit-conversion layer ([`pantry::convert_str`]).
pub trait MealNutrition {
    /// Total nutrition for this meal — computed by
    /// matching each recipe's cooklang `@ingredient{qty%unit}`
    /// lines against `pantry`'s per-unit nutrition data.
    /// The pantry / wiki page for each ingredient (e.g.
    /// `Wiki/flour.md`) carries `nutritionPerUnit`; we sum
    /// `qty × per_unit_nutrition / unit_basis` for every
    /// ingredient that resolves.
    ///
    /// `recipes` is any superset that includes this meal's
    /// referenced recipes (by path); unmatched paths are
    /// skipped. `pantry` is the current pantry snapshot.
    /// Returns `None` when no ingredient resolves.
    fn nutrition_total(
        &self,
        recipes: &[cookbook::Recipe],
        pantry: &[pantry::PantryItem],
    ) -> Option<cookbook::Nutrition>;
}

impl MealNutrition for Meal {
    fn nutrition_total(
        &self,
        recipes: &[cookbook::Recipe],
        pantry: &[pantry::PantryItem],
    ) -> Option<cookbook::Nutrition> {
        use std::collections::HashMap;
        let index: HashMap<&str, &cookbook::Recipe> =
            recipes.iter().map(|r| (r.path.as_str(), r)).collect();

        let mut acc = cookbook::Nutrition::default();
        let mut any = false;
        for path in self.recipe_paths.iter() {
            let Some(recipe) = index.get(path.as_str()) else {
                continue;
            };
            let base = recipe.servings.filter(|s| *s > 0).unwrap_or(1);
            let scale = f64::from(self.servings) / f64::from(base);
            for ing in recipe.ingredients.iter() {
                let Some(qty) = ing.qty else {
                    continue;
                };
                let Some(item) = match_pantry_by_name(&ing.name, pantry) else {
                    continue;
                };
                let Some(per_unit) = &item.nutrition_per_unit else {
                    continue;
                };
                // nutritionUnit on the pantry item is the
                // basis (e.g. "100g" or "1 cup"). We compute
                // factor = (qty * scale, converted to item.unit)
                //          / basis_amount
                let item_qty =
                    pantry::convert_str(qty * scale, &ing.unit, &item.unit).unwrap_or(qty * scale);
                let basis = nutrition_basis_qty(item.nutrition_unit.as_deref().unwrap_or(""));
                if basis <= 0.0 {
                    continue;
                }
                let factor = item_qty / basis;
                any = true;
                acc.calories = sum(acc.calories, per_unit.calories.map(|v| v * factor));
                acc.protein_g = sum(acc.protein_g, per_unit.protein_g.map(|v| v * factor));
                acc.carbs_g = sum(acc.carbs_g, per_unit.carbs_g.map(|v| v * factor));
                acc.fat_g = sum(acc.fat_g, per_unit.fat_g.map(|v| v * factor));
                acc.fiber_g = sum(acc.fiber_g, per_unit.fiber_g.map(|v| v * factor));
                acc.sugar_g = sum(acc.sugar_g, per_unit.sugar_g.map(|v| v * factor));
            }
        }
        any.then_some(acc)
    }
}

fn match_pantry_by_name<'a>(
    needle_raw: &str,
    pantry: &'a [pantry::PantryItem],
) -> Option<&'a pantry::PantryItem> {
    let needle = needle_raw
        .trim()
        .trim_start_matches("[[")
        .trim_end_matches("]]")
        .to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    pantry
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(&needle))
        .or_else(|| {
            pantry
                .iter()
                .find(|p| p.name.to_ascii_lowercase().contains(&needle))
        })
        .or_else(|| {
            pantry
                .iter()
                .find(|p| needle.contains(&p.name.to_ascii_lowercase()))
        })
}

/// Parse the numeric basis from a `nutritionUnit` field like
/// `"100g"`, `"1 cup"`, `"30 ml"`. Returns the numeric prefix;
/// `1.0` when none parses (treat as a per-unit-of-stock basis).
fn nutrition_basis_qty(unit: &str) -> f64 {
    let trimmed = unit.trim();
    if trimmed.is_empty() {
        return 1.0;
    }
    let n: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    n.parse().unwrap_or(1.0)
}

fn sum(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}
