//! Recipe fulfillment — "can I make this from what's in
//! the pantry?". Pure functions over a parsed cooklang
//! recipe + the pantry snapshot.
//!
//! The match flow: each recipe ingredient (cooklang
//! `@name{qty%unit}`) is paired to a `pantry::PantryItem`
//! by **name** (case-insensitive, substring fallback). The
//! recipe carries no pantry IDs — cooklang files are pure
//! and portable, so the join is at the name layer. The
//! ingredient's qty + unit is compared to the pantry item's
//! `stock_total` + `unit` with unit conversion via
//! [`pantry::convert_str`].
//!
//! Output: [`Fulfillment { can_cook, missing }`] where
//! `missing` carries the per-ingredient shortfall so the
//! shopping-list builder can populate against it.
//!
//! Substitutions: see [`check_with_subs`] — pulls from the
//! pantry-item sub layer and the registry rule layer.
//! Recipe-level subs no longer exist (cooklang has no
//! native concept); encode them as pantry-side substitutes
//! or registry rules.

use cookbook::Recipe;
use pantry::PantryItem;

// The fulfillment **wire types** now live in the wasm-clean
// `mealplan-proto` crate (so `MealplanService::can_cook` can
// return `Fulfillment` from a wasm client). The pure matching
// logic below stays here — it needs the native `pantry`
// unit-conversion layer. Re-exported so existing
// `mealplan::fulfillment::*` / `mealplan::Fulfillment` paths keep
// working.
pub use mealplan_proto::fulfillment::{
    CookReceipt, DeductionLine, Fulfillment, HaveLine, Shortage, ShortageReason, SkipReason,
    SkippedIngredient, SubstitutionSource, SubstitutionSuggestion,
};

const MAX_NEST_DEPTH: u32 = 8;

#[must_use]
pub fn check(recipe: &Recipe, pantry: &[PantryItem], servings: u32) -> Fulfillment {
    let scale = scale_factor(recipe, servings);
    let mut missing = Vec::new();
    for (idx, ing) in recipe.ingredients.iter().enumerate() {
        check_one(ing, idx, scale, pantry, &mut missing);
    }
    // `have` is the complement of `missing`: every non-recipe-ref
    // ingredient the check didn't flag short. Computed here, in the
    // domain, so the CLI and UI both render it without re-deriving.
    let short_idx: std::collections::HashSet<u32> =
        missing.iter().map(|s| s.ingredient_idx).collect();
    let have = recipe
        .ingredients
        .iter()
        .enumerate()
        .filter(|(idx, ing)| !ing.is_recipe_ref && !short_idx.contains(&(*idx as u32)))
        .map(|(_, ing)| HaveLine {
            name: ing.name.clone(),
            need: ing.qty.map(|q| q * scale),
            unit: ing.unit.clone(),
        })
        .collect();
    Fulfillment {
        can_cook: missing.iter().all(|s| {
            !matches!(
                s.reason,
                ShortageReason::NotInPantry | ShortageReason::InsufficientQty
            )
        }),
        have,
        missing,
    }
}

/// A full cook receipt for `recipe` at `servings`: for each ingredient
/// matched to a pantry item with a convertible unit, the amount to
/// consume **in the pantry item's unit**, capped at what's in stock.
/// Cooking never invents a deduction it can't compute safely — but
/// instead of silently dropping those ingredients it records each in
/// [`CookReceipt::skipped`] with the reason (no quantity, no pantry
/// match, inconvertible unit, or out of stock) so the cook can see what
/// to top up by hand. The `deducted` rows feed `PantryService::consume`.
#[must_use]
pub fn plan_cook(recipe: &Recipe, pantry: &[PantryItem], servings: u32) -> CookReceipt {
    let scale = scale_factor(recipe, servings);
    let mut receipt = CookReceipt::default();
    for ing in recipe.ingredients.iter() {
        // Resolve the deduction (pushed inline), or the reason it
        // can't be made. `None` ⇒ deducted; `Some(reason)` ⇒ skipped.
        let skip = 'plan: {
            let Some(need) = ing.qty.map(|q| q * scale) else {
                break 'plan Some(SkipReason::NoQuantity);
            };
            let Some(item) = match_pantry(ing, pantry) else {
                break 'plan Some(SkipReason::NoPantryMatch);
            };
            let Some(in_item_unit) = pantry::convert_str(need, &ing.unit, &item.unit) else {
                break 'plan Some(SkipReason::InconvertibleUnit);
            };
            let available = item.stock_total().unwrap_or(0.0);
            let qty = in_item_unit.min(available);
            if qty <= 1e-9 {
                break 'plan Some(SkipReason::OutOfStock);
            }
            receipt.deducted.push(DeductionLine {
                item_id: item.id,
                ingredient: ing.name.clone(),
                qty,
                unit: item.unit.clone(),
            });
            None
        };
        if let Some(reason) = skip {
            receipt.skipped.push(SkippedIngredient {
                ingredient: ing.name.clone(),
                reason,
            });
        }
    }
    receipt
}

/// The pantry deductions for cooking `recipe` at `servings`, as the
/// plain `PantryDeduction` rows the meal-`cook` path stamps onto a
/// meal. A thin projection of [`plan_cook`] — see it for the matching
/// rules and the skipped-ingredient accounting.
#[must_use]
pub fn plan_deductions(
    recipe: &Recipe,
    pantry: &[PantryItem],
    servings: u32,
) -> Vec<crate::model::PantryDeduction> {
    plan_cook(recipe, pantry, servings)
        .deducted
        .into_iter()
        .map(|line| crate::model::PantryDeduction {
            item_id: line.item_id,
            qty: line.qty,
            unit: line.unit,
        })
        .collect()
}

fn scale_factor(recipe: &Recipe, servings: u32) -> f64 {
    let base = recipe.servings.filter(|s| *s > 0).unwrap_or(1);
    f64::from(servings) / f64::from(base)
}

fn check_one(
    ing: &cookbook::Ingredient,
    idx: usize,
    scale: f64,
    pantry: &[PantryItem],
    missing: &mut Vec<Shortage>,
) {
    let need = match ing.qty {
        Some(q) => q * scale,
        None if ing.optional => return,
        None => {
            missing.push(Shortage {
                name: ing.name.clone(),
                ingredient_idx: idx as u32,
                need: 0.0,
                have: 0.0,
                unit: ing.unit.clone(),
                reason: ShortageReason::OptionalNoQty,
                suggestions: Vec::new(),
            });
            return;
        }
    };

    match match_pantry(ing, pantry) {
        Some(item) => {
            let have_total = item.stock_total().unwrap_or(0.0);
            let have = pantry::convert_str(have_total, &item.unit, &ing.unit);
            match have {
                Some(h) if h + 1e-9 >= need => {}
                Some(h) => missing.push(Shortage {
                    name: ing.name.clone(),
                    ingredient_idx: idx as u32,
                    need,
                    have: h,
                    unit: ing.unit.clone(),
                    reason: ShortageReason::InsufficientQty,
                    suggestions: Vec::new(),
                }),
                None => missing.push(Shortage {
                    name: ing.name.clone(),
                    ingredient_idx: idx as u32,
                    need,
                    have: have_total,
                    unit: ing.unit.clone(),
                    reason: ShortageReason::UnitMismatch,
                    suggestions: Vec::new(),
                }),
            }
        }
        None if ing.optional => {}
        None => missing.push(Shortage {
            name: ing.name.clone(),
            ingredient_idx: idx as u32,
            need,
            have: 0.0,
            unit: ing.unit.clone(),
            reason: ShortageReason::NotInPantry,
            suggestions: Vec::new(),
        }),
    }
}

/// Check fulfillment with nested-recipe support. Recurses
/// through `Recipe::nested_recipes` (cooklang `@@./path{}`)
/// up to [`MAX_NEST_DEPTH`] levels. `all_recipes` must
/// include every nested recipe by `path`; missing paths are
/// silently skipped.
#[must_use]
pub fn check_nested(
    recipe: &Recipe,
    all_recipes: &[Recipe],
    pantry: &[PantryItem],
    servings: u32,
) -> Fulfillment {
    use std::collections::HashMap;
    let index: HashMap<&str, &Recipe> = all_recipes.iter().map(|r| (r.path.as_str(), r)).collect();

    let scale = scale_factor(recipe, servings);
    let mut visited = std::collections::HashSet::new();
    let mut flat = flatten(recipe, &index, scale, &mut visited, 0);
    fold_same_ingredient(&mut flat);

    let synthetic = Recipe {
        ingredients: cookbook::Ingredients(flat),
        servings: Some(1),
        ..recipe.clone()
    };
    check(&synthetic, pantry, 1)
}

fn flatten(
    recipe: &Recipe,
    index: &std::collections::HashMap<&str, &Recipe>,
    scale: f64,
    visited: &mut std::collections::HashSet<String>,
    depth: u32,
) -> Vec<cookbook::Ingredient> {
    if depth > MAX_NEST_DEPTH || visited.contains(&recipe.path) {
        return Vec::new();
    }
    visited.insert(recipe.path.clone());

    // Resolve first: an unresolved reference has to stay in the
    // ingredient list as an opaque row, or the sauce vanishes from the
    // shopping list entirely instead of showing up as "buy this".
    let resolved: Vec<(String, &Recipe)> = recipe
        .nested_recipes
        .iter()
        .filter_map(|nested| {
            resolve_ref(nested, &recipe.path, index).map(|child| (ref_stem(nested), child))
        })
        .collect();

    let mut out: Vec<cookbook::Ingredient> = recipe
        .ingredients
        .iter()
        // A `@@sauce` reference is also listed as an ingredient. Once
        // it's expanded into what the sauce is actually made of, keeping
        // the row too would count it twice.
        .filter(|ing| {
            !(ing.is_recipe_ref
                && resolved
                    .iter()
                    .any(|(stem, _)| stem.eq_ignore_ascii_case(&ing.name)))
        })
        .map(|ing| cookbook::Ingredient {
            qty: ing.qty.map(|q| q * scale),
            ..ing.clone()
        })
        .collect();

    for (stem, child) in resolved {
        let base = f64::from(child.servings.unwrap_or(1).max(1));
        // `[[Sauce]]{2}` calls for two of the sauce's servings. With no
        // quantity — `[[Sauce]]{}` — you're making the whole thing,
        // which is what a component recipe usually means: one batch of
        // dough, one batch of sauce.
        let want = recipe
            .ingredients
            .iter()
            .find(|ing| ing.is_recipe_ref && stem.eq_ignore_ascii_case(&ing.name))
            .and_then(|ing| ing.qty)
            .unwrap_or(base);
        let child_scale = (want * scale) / base;
        out.extend(flatten(child, index, child_scale, visited, depth + 1));
    }

    visited.remove(&recipe.path);
    out
}

/// The bare name a reference ends in — `./Cookbook/hot-honey` →
/// `hot-honey`. This is what cooklang also uses as the reference
/// ingredient's display name, so it's how the two get paired up.
fn ref_stem(reference: &str) -> String {
    reference
        .trim_start_matches("./")
        .rsplit('/')
        .next()
        .unwrap_or(reference)
        .trim_end_matches(".cook")
        .to_string()
}

/// Resolve a cooklang recipe reference against the recipe index.
///
/// Cooklang hands back the reference exactly as written — `@@./sauce`
/// becomes `./sauce` — while recipes are indexed by vault path
/// (`Cookbook/sauce.cook`). Those never match, so before this every
/// reference silently resolved to nothing: the child's ingredients
/// never reached a pantry check or a shopping list, and no error said
/// so. Try the literal string, then the reference resolved relative to
/// the referring recipe's own folder, then a bare filename, and
/// finally a case-insensitive match on file stem or display name.
fn resolve_ref<'a>(
    reference: &str,
    referrer_path: &str,
    index: &std::collections::HashMap<&str, &'a Recipe>,
) -> Option<&'a Recipe> {
    if let Some(found) = index.get(reference) {
        return Some(found);
    }

    let rel = reference.trim_start_matches("./");
    let dir = referrer_path.rsplit_once('/').map_or("", |(d, _)| d);
    let mut candidates = Vec::with_capacity(4);
    if !dir.is_empty() {
        candidates.push(format!("{dir}/{rel}.cook"));
        candidates.push(format!("{dir}/{rel}"));
    }
    candidates.push(format!("{rel}.cook"));
    candidates.push(rel.to_string());
    for candidate in &candidates {
        if let Some(found) = index.get(candidate.as_str()) {
            return Some(found);
        }
    }

    // Last resort, so `@@./Nashville Hot Honey` finds
    // `Cookbook/nashville-hot-honey.cook`.
    let want = ref_stem(reference);
    index.values().copied().find(|r| {
        ref_stem(&r.path).eq_ignore_ascii_case(&want) || r.name.eq_ignore_ascii_case(&want)
    })
}

/// Combine repeated mentions of the same ingredient into one row.
///
/// Units are reconciled through cooklang's own unit database rather
/// than by comparing strings, so a recipe calling for 500 g of flour
/// and a sub-recipe calling for 0.5 kg produce a single 1 kg row
/// instead of two that each look short. Amounts land in the unit of
/// the first mention, which is the one the reader has already seen.
///
/// Rows whose units genuinely don't reconcile — grams against cloves —
/// stay separate. That is information, not a failure: nothing sensible
/// can be added there, and silently picking one would be a lie.
fn fold_same_ingredient(rows: &mut Vec<cookbook::Ingredient>) {
    let mut i = 0;
    while i < rows.len() {
        let mut j = i + 1;
        while j < rows.len() {
            if !rows[i].name.eq_ignore_ascii_case(&rows[j].name) {
                j += 1;
                continue;
            }
            // Bring j into i's unit before adding. A missing quantity
            // contributes nothing but still folds away, so "salt" twice
            // is one line.
            let add = match rows[j].qty {
                None => Some(0.0),
                Some(q) => cookbook::convert(q, &rows[j].unit, &rows[i].unit),
            };
            match add {
                Some(add) => {
                    let base = rows[i].qty.unwrap_or(0.0);
                    rows[i].qty = Some(base + add);
                    // A merged row is a sum, not a range any more.
                    rows[i].qty_max = None;
                    rows.remove(j);
                }
                None => j += 1,
            }
        }
        i += 1;
    }
}

#[must_use]
pub fn check_with_subs(
    recipe: &Recipe,
    pantry: &[PantryItem],
    rules: &[crate::substitutions::SubstitutionRule],
    goals: &[pantry::SubReason],
) -> Fulfillment {
    let mut base = check(recipe, pantry, recipe.servings.unwrap_or(1));
    let pantry_by_id: std::collections::HashMap<uuid::Uuid, &PantryItem> =
        pantry.iter().map(|p| (p.id, p)).collect();

    for short in &mut base.missing {
        let idx = short.ingredient_idx as usize;
        let Some(ing) = recipe.ingredients.get(idx) else {
            continue;
        };
        let Some(primary) = match_pantry(ing, pantry) else {
            continue;
        };

        for sub in primary.substitutes.iter() {
            if let Some(target) = pantry_by_id.get(&sub.item_id) {
                let need = short.need * sub.ratio;
                let have = pantry::convert_str(
                    target.stock_total().unwrap_or(0.0),
                    &target.unit,
                    &short.unit,
                );
                short.suggestions.push(SubstitutionSuggestion {
                    name: target.name.clone(),
                    to_item_id: Some(target.id),
                    ratio: sub.ratio,
                    need,
                    have,
                    reasons: sub.reasons.clone(),
                    source: SubstitutionSource::PantryItem,
                    note: sub.note.clone(),
                });
            }
        }

        for rule in rules.iter().filter(|r| r.from_item_id == primary.id) {
            if let Some(target) = pantry_by_id.get(&rule.to_item_id) {
                let need = short.need * rule.ratio;
                let have = pantry::convert_str(
                    target.stock_total().unwrap_or(0.0),
                    &target.unit,
                    &short.unit,
                );
                short.suggestions.push(SubstitutionSuggestion {
                    name: target.name.clone(),
                    to_item_id: Some(target.id),
                    ratio: rule.ratio,
                    need,
                    have,
                    reasons: rule.reasons.0.clone(),
                    source: SubstitutionSource::Registry,
                    note: rule.note.clone(),
                });
            }
        }

        if goals.is_empty() {
            short.suggestions.sort_by_key(|s| {
                let oos = !s.reasons.contains(&pantry::SubReason::OutOfStock);
                let layer = match s.source {
                    SubstitutionSource::PantryItem => 0,
                    SubstitutionSource::Registry => 1,
                };
                (oos, layer)
            });
        } else {
            short
                .suggestions
                .retain(|s| s.reasons.iter().any(|r| goals.contains(r)));
            short.suggestions.sort_by(|a, b| {
                let a_score = a.reasons.iter().filter(|r| goals.contains(r)).count();
                let b_score = b.reasons.iter().filter(|r| goals.contains(r)).count();
                b_score.cmp(&a_score)
            });
        }
    }

    base
}

fn match_pantry<'p>(
    ing: &cookbook::Ingredient,
    pantry: &'p [PantryItem],
) -> Option<&'p PantryItem> {
    let needle = ing
        .name
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use cookbook::Ingredient;
    use uuid::Uuid;

    fn pantry_row(name: &str, qty: f64, unit: &str) -> PantryItem {
        let mut row = pantry::PantryItem::from_item(inventory::Item {
            path: String::new(),
            id: Uuid::new_v4(),
            name: name.to_string(),
            category: "food".into(),
            location_id: None,
            condition: "good".into(),
            status: "stored".into(),
            manufacturer: None,
            model: None,
            serial: None,
            purchase_date: None,
            value: None,
            tasks: inventory::model::StringList::default(),
            tags: inventory::model::StringList(vec!["item".into(), "pantry".into()]),
            date_created: None,
            date_modified: None,
            details: String::new(),
        });
        row.qty = Some(qty);
        row.unit = unit.to_string();
        row.stock_entries.push(pantry::StockEntry {
            id: Uuid::new_v4(),
            qty,
            purchased_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            best_before: None,
            opened: false,
            opened_date: None,
            price: None,
            location_id: None,
            note: None,
        });
        row
    }

    fn recipe_with(path: &str, ings: Vec<Ingredient>, servings: u32) -> Recipe {
        Recipe {
            cook_steps: Default::default(),
            path: path.into(),
            name: "test".into(),
            description: None,
            course: None,
            cuisine: None,
            prep_minutes: None,
            cook_minutes: None,
            servings: Some(servings),
            ingredients: cookbook::Ingredients(ings),
            steps: cookbook::StringList::default(),
            cookware: cookbook::StringList::default(),
            nested_recipes: cookbook::StringList::default(),
            tags: cookbook::StringList::default(),
            source_url: None,
            date_modified: None,
            source: String::new(),
            images: Default::default(),
        }
    }

    fn ing(name: &str, qty: f64, unit: &str) -> Ingredient {
        Ingredient {
            name: name.into(),
            alias: None,
            qty: Some(qty),
            qty_max: None,
            unit: unit.into(),
            qty_display: None,
            scalable: true,
            note: None,
            optional: false,
            is_recipe_ref: false,
        }
    }

    #[test]
    fn can_cook_when_stock_sufficient() {
        let r = recipe_with("Cookbook/X.cook", vec![ing("Pasta", 200.0, "g")], 2);
        let s = vec![pantry_row("Pasta", 500.0, "g")];
        let f = check(&r, &s, 2);
        assert!(f.can_cook);
        // The satisfied ingredient lands in `have` (the domain owns the
        // have/missing partition; the CLI + UI just render it).
        assert_eq!(f.have.len(), 1);
        assert_eq!(f.have[0].name, "Pasta");
        assert_eq!(f.have[0].need, Some(200.0));
        assert!(f.missing.is_empty());
    }

    #[test]
    fn have_and_missing_partition_ingredients() {
        let r = recipe_with(
            "Cookbook/X.cook",
            vec![
                ing("Pasta", 200.0, "g"), // in stock → have
                ing("Saffron", 1.0, "g"), // not in pantry → missing
            ],
            2,
        );
        let s = vec![pantry_row("Pasta", 500.0, "g")];
        let f = check(&r, &s, 2);
        assert_eq!(
            f.have.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            ["Pasta"]
        );
        assert_eq!(
            f.missing
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["Saffron"]
        );
    }

    #[test]
    fn plan_deductions_scales_caps_and_skips() {
        let r = recipe_with(
            "Cookbook/X.cook",
            vec![
                ing("Pasta", 200.0, "g"), // matched, in stock
                ing("Salt", 5.0, "g"),    // matched but only 2g in stock → capped
                ing("Saffron", 1.0, "g"), // not in pantry → skipped
            ],
            2,
        );
        let pantry = vec![
            pantry_row("Pasta", 500.0, "g"),
            pantry_row("Salt", 2.0, "g"),
        ];

        // Doubling the servings (base 2 → 4) doubles the needs.
        let plan = plan_deductions(&r, &pantry, 4);
        assert_eq!(plan.len(), 2, "saffron has no pantry match → no deduction");

        let pasta = plan.iter().find(|d| (d.qty - 400.0).abs() < 1e-6);
        assert!(pasta.is_some(), "200g × 2 servings = 400g, 500g in stock");

        // 5g × 2 = 10g needed, but only 2g on hand → capped at 2g.
        let salt = plan.iter().find(|d| (d.qty - 2.0).abs() < 1e-6);
        assert!(salt.is_some(), "deduction is capped at available stock");
    }

    #[test]
    fn plan_cook_surfaces_skipped_ingredients() {
        let mut to_taste = ing("Pepper", 0.0, "g");
        to_taste.qty = None; // "@pepper" with no amount
        let r = recipe_with(
            "Cookbook/X.cook",
            vec![
                ing("Pasta", 200.0, "g"),     // matched, in stock → deducted
                ing("Salt", 5.0, "g"),        // matched but 0g on hand → out of stock
                ing("Garlic", 2.0, "cloves"), // matched but unit won't convert
                ing("Saffron", 1.0, "g"),     // no pantry match
                to_taste,                     // no quantity
            ],
            2,
        );
        let pantry = vec![
            pantry_row("Pasta", 500.0, "g"),
            pantry_row("Salt", 0.0, "g"),
            pantry_row("Garlic", 100.0, "g"),
        ];

        let receipt = plan_cook(&r, &pantry, 2);

        assert_eq!(receipt.deducted.len(), 1, "only pasta is deductible");
        assert_eq!(receipt.deducted[0].ingredient, "Pasta");
        assert!((receipt.deducted[0].qty - 200.0).abs() < 1e-6);

        let reason = |name: &str| {
            receipt
                .skipped
                .iter()
                .find(|s| s.ingredient == name)
                .map(|s| s.reason)
        };
        assert_eq!(reason("Salt"), Some(SkipReason::OutOfStock));
        assert_eq!(reason("Garlic"), Some(SkipReason::InconvertibleUnit));
        assert_eq!(reason("Saffron"), Some(SkipReason::NoPantryMatch));
        assert_eq!(reason("Pepper"), Some(SkipReason::NoQuantity));
    }

    #[test]
    fn scales_with_servings() {
        let r = recipe_with("Cookbook/X.cook", vec![ing("Pasta", 200.0, "g")], 2);
        let s = vec![pantry_row("Pasta", 300.0, "g")];
        let f = check(&r, &s, 4);
        assert!(!f.can_cook);
        assert!(matches!(
            f.missing[0].reason,
            ShortageReason::InsufficientQty
        ));
    }

    #[test]
    fn cross_unit_with_conversion() {
        let r = recipe_with("Cookbook/X.cook", vec![ing("Olive Oil", 30.0, "ml")], 1);
        let s = vec![pantry_row("Olive Oil", 1.0, "l")];
        assert!(check(&r, &s, 1).can_cook);
    }

    #[test]
    fn missing_item_surfaces() {
        let r = recipe_with("Cookbook/X.cook", vec![ing("Truffles", 5.0, "g")], 1);
        let s = vec![pantry_row("Pasta", 500.0, "g")];
        let f = check(&r, &s, 1);
        assert!(matches!(f.missing[0].reason, ShortageReason::NotInPantry));
    }

    #[test]
    fn optional_ingredient_doesnt_block() {
        let mut opt = ing("Truffles", 5.0, "g");
        opt.optional = true;
        let r = recipe_with("Cookbook/X.cook", vec![ing("Pasta", 100.0, "g"), opt], 1);
        let s = vec![pantry_row("Pasta", 500.0, "g")];
        assert!(check(&r, &s, 1).can_cook);
    }

    #[test]
    fn nested_recipe_aggregates_ingredients() {
        let mut dough = recipe_with("Cookbook/Dough.cook", vec![ing("Flour", 200.0, "g")], 1);
        dough.name = "Pizza Dough".into();
        let mut pizza = recipe_with("Cookbook/Pizza.cook", vec![ing("Flour", 100.0, "g")], 1);
        pizza.name = "Pizza".into();
        pizza.nested_recipes = cookbook::StringList(vec!["Cookbook/Dough.cook".into()]);
        let s = vec![pantry_row("Flour", 250.0, "g")];
        let f = check_nested(&pizza, &[pizza.clone(), dough], &s, 1);
        assert!(!f.can_cook);
        assert!((f.missing[0].need - 300.0).abs() < 1e-6);
    }

    #[test]
    fn cycle_guard() {
        let mut a = recipe_with("Cookbook/A.cook", vec![ing("X", 1.0, "g")], 1);
        let mut b = recipe_with("Cookbook/B.cook", vec![ing("Y", 1.0, "g")], 1);
        a.nested_recipes = cookbook::StringList(vec!["Cookbook/B.cook".into()]);
        b.nested_recipes = cookbook::StringList(vec!["Cookbook/A.cook".into()]);
        let s = vec![pantry_row("X", 10.0, "g"), pantry_row("Y", 10.0, "g")];
        assert!(check_nested(&a, &[a.clone(), b.clone()], &s, 1).can_cook);
    }
}
