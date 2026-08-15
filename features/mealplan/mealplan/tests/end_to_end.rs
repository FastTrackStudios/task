//! End-to-end smoke test for the full mealplan slice.
//!
//! 1. Open an empty tempdir vault.
//! 2. Create three pantry items (pasta + olive oil + butter)
//!    — pasta + oil with stock, butter empty + carrying
//!    `nutritionPerUnit` so the recipe nutrition aggregator
//!    has something to sum.
//! 3. Write a cooklang `.cook` recipe that uses
//!    `@pasta`, `@olive oil`, `@butter`, `@truffles`.
//! 4. Create a meal referencing the recipe by path + cook it
//!    (debits pasta + oil from pantry).
//! 5. Check fulfillment for a follow-up meal — confirms
//!    truffles shortage + butter sub suggestion surface.
//! 6. Auto-populate a shopping list from the missing items.
//! 7. Confirm meal nutrition aggregates from pantry data.

use chrono::NaiveDate;
use cookbook::{CookbookService, Nutrition, Recipe};
use mealplan::{
    MealNutrition, MealplanService, ShoppingService, ShoppingStore, ShortageReason, Slot,
    SubstitutionService, SubstitutionStore, fulfillment,
};
use pantry::{PantryService, StockEntry, SubReason};
use std::path::Path;
use tempfile::tempdir;
use uuid::Uuid;
use vault::Vault;

fn fresh_vault(root: &Path) -> Vault {
    std::fs::create_dir_all(root.join("Operations/Inventory/Pantry")).unwrap();
    std::fs::create_dir_all(root.join("Cookbook")).unwrap();
    std::fs::create_dir_all(root.join("Projects/Mealplan/meals")).unwrap();
    std::fs::create_dir_all(root.join("shopping")).unwrap();
    std::fs::create_dir_all(root.join("substitutions")).unwrap();
    Vault::open(root).expect("open vault")
}

fn item(name: &str, unit: &str) -> pantry::PantryItem {
    let mut p = pantry::PantryItem::from_item(inventory::Item {
        path: String::new(),
        id: Uuid::nil(),
        name: name.into(),
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
    p.unit = unit.into();
    p
}

#[test]
fn full_grocery_to_meal_loop() {
    let dir = tempdir().unwrap();
    let vault = fresh_vault(dir.path());
    let root = vault.root.clone();

    let store = mealplan::Store::new(vault);
    let pantry = store.pantry().clone();
    let cookbook = store.cookbook().clone();
    let shopping = ShoppingStore::from_shared(store.shared());
    let subs = SubstitutionStore::from_shared(store.shared());

    // ── 1. Pantry items + stock ──
    let mut pasta = item("Pasta", "g");
    pasta.barcodes = pantry::model::StringList(vec!["8001234567890".into()]);
    pasta.nutrition_per_unit = Some(Nutrition {
        calories: Some(371.0),
        protein_g: Some(13.0),
        carbs_g: Some(75.0),
        fat_g: Some(1.5),
        fiber_g: None,
        sugar_g: None,
    });
    pasta.nutrition_unit = Some("100g".into());
    let pasta = pantry.create(pasta).unwrap();
    pantry
        .add_stock(
            &pasta.id.to_string(),
            StockEntry {
                id: Uuid::nil(),
                qty: 500.0,
                purchased_date: NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
                best_before: None,
                opened: false,
                opened_date: None,
                price: Some(2.49),
                location_id: None,
                note: None,
            },
        )
        .unwrap();

    let mut olive_oil = item("Olive Oil", "ml");
    olive_oil.nutrition_per_unit = Some(Nutrition {
        calories: Some(884.0),
        protein_g: Some(0.0),
        carbs_g: Some(0.0),
        fat_g: Some(100.0),
        fiber_g: None,
        sugar_g: None,
    });
    olive_oil.nutrition_unit = Some("100ml".into());
    let olive_oil = pantry.create(olive_oil).unwrap();
    pantry
        .add_stock(
            &olive_oil.id.to_string(),
            StockEntry {
                id: Uuid::nil(),
                qty: 1000.0,
                purchased_date: NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
                best_before: None,
                opened: false,
                opened_date: None,
                price: Some(8.99),
                location_id: None,
                note: None,
            },
        )
        .unwrap();

    let butter = pantry.create(item("Butter", "g")).unwrap();

    // ── 2. Substitution rule: butter → olive oil ──
    subs.create(mealplan::SubstitutionRule {
        path: String::new(),
        id: Uuid::nil(),
        name: "Butter → Olive Oil".into(),
        from_item_id: butter.id,
        to_item_id: olive_oil.id,
        ratio: 0.75,
        reasons: mealplan::substitutions::SubReasons(vec![SubReason::Vegan, SubReason::OutOfStock]),
        tags: mealplan::model::StringList::default(),
        note: None,
        date_created: None,
        date_modified: None,
        details: String::new(),
    })
    .unwrap();

    // ── 3. Cooklang recipe ──
    let recipe_src = "\
>> title: Truffle Pasta
>> course: dinner
>> servings: 2
>> prep time: 5 minutes
>> cook time: 15 minutes
>> tags: weeknight

Bring a pot of water to a boil and cook @pasta{200%g} until al dente.
Meanwhile, warm @olive oil{30%ml} in a pan.
Toss with @butter{20%g} and shaved @truffles{5%g}.
";
    let recipe_path = "Cookbook/Truffle Pasta.cook";
    let mut draft = Recipe {
        cook_steps: Default::default(),
        path: recipe_path.into(),
        name: "Truffle Pasta".into(),
        description: None,
        course: None,
        cuisine: None,
        prep_minutes: None,
        cook_minutes: None,
        servings: None,
        ingredients: cookbook::Ingredients::default(),
        steps: cookbook::StringList::default(),
        cookware: cookbook::StringList::default(),
        nested_recipes: cookbook::StringList::default(),
        tags: cookbook::StringList::default(),
        source_url: None,
        date_modified: None,
        source: recipe_src.into(),
        images: Default::default(),
    };
    draft = cookbook.create(draft).unwrap();
    assert_eq!(draft.servings, Some(2));
    assert_eq!(draft.ingredients.len(), 4);

    // ── 4. Schedule + cook a meal ──
    let meal = mealplan::Meal {
        path: String::new(),
        id: Uuid::nil(),
        name: "Friday Pasta Night".into(),
        scheduled_for: NaiveDate::from_ymd_opt(2026, 5, 22).unwrap(),
        slot: Slot::Dinner.as_str().to_string(),
        servings: 2,
        recipe_paths: mealplan::model::StringList(vec![recipe_path.into()]),
        status: mealplan::Status::Planned.as_str().to_string(),
        pantry_deductions: mealplan::model::PantryDeductions::default(),
        tags: mealplan::model::StringList::default(),
        date_created: None,
        date_modified: None,
        details: String::new(),
    };
    let meal = store.create(meal).unwrap();
    let cooked = store
        .cook(
            &meal.id.to_string(),
            vec![
                mealplan::PantryDeduction {
                    item_id: pasta.id,
                    qty: 200.0,
                    unit: "g".into(),
                },
                mealplan::PantryDeduction {
                    item_id: olive_oil.id,
                    qty: 30.0,
                    unit: "ml".into(),
                },
            ],
        )
        .unwrap();
    assert_eq!(cooked.status, "cooked");

    let pasta_now = pantry.get(&pasta.id.to_string()).unwrap();
    assert!((pasta_now.stock_total().unwrap() - 300.0).abs() < 1e-6);
    let oil_now = pantry.get(&olive_oil.id.to_string()).unwrap();
    assert!((oil_now.stock_total().unwrap() - 970.0).abs() < 1e-6);

    // ── 5. Fulfillment with subs ──
    let rules = subs.list().unwrap();
    let pantry_now = pantry.list().unwrap();
    let recipe = cookbook.get(recipe_path).unwrap();
    let f = fulfillment::check_with_subs(&recipe, &pantry_now, &rules, &[]);
    assert!(!f.can_cook);

    let by_name: std::collections::HashMap<String, &mealplan::Shortage> = f
        .missing
        .iter()
        .map(|s| (s.name.to_ascii_lowercase(), s))
        .collect();
    let truffles = by_name.get("truffles").expect("truffles shortage");
    assert!(matches!(truffles.reason, ShortageReason::NotInPantry));
    let butter_short = by_name.get("butter").expect("butter shortage");
    assert!(
        !butter_short.suggestions.is_empty(),
        "butter should have subs from registry rule"
    );
    assert!(
        butter_short
            .suggestions
            .iter()
            .any(|s| s.name == "Olive Oil"),
        "registry rule butter→olive-oil should surface"
    );

    // ── 6. Shopping list auto-populate ──
    let list = shopping
        .create(mealplan::ShoppingList {
            path: String::new(),
            id: Uuid::nil(),
            name: "Weekly Grocery Run".into(),
            store_location_id: None,
            entries: mealplan::shopping::ShoppingEntries::default(),
            is_template: false,
            from_template: None,
            date_created: None,
            date_modified: None,
            details: String::new(),
        })
        .unwrap();
    let list = shopping
        .add_missing_for_recipe(&list.id.to_string(), recipe_path, 2)
        .unwrap();
    assert!(
        list.entries
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case("truffles")),
        "shopping list should pick up truffles"
    );
    assert!(
        list.entries
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case("butter")),
        "shopping list should pick up butter"
    );

    // ── 7. Nutrition aggregation from pantry data ──
    // Per-meal: 2 servings of a 2-serving recipe = 1x ingredient qtys.
    // Pasta 200g × 371kcal/100g = 742 kcal; oil 30ml × 884kcal/100ml = 265.2 kcal.
    // Total ≈ 1007.2 kcal. Butter + truffles unmatched (no per-unit data) → skipped.
    let recipes = vec![recipe];
    let pantry_now = pantry.list().unwrap();
    let total = cooked
        .nutrition_total(&recipes, &pantry_now)
        .expect("nutrition");
    let cal = total.calories.unwrap();
    assert!(
        (cal - 1007.2).abs() < 1.0,
        "expected ~1007.2 calories, got {cal}"
    );

    // Sanity that the vault root we constructed is what the store sees.
    assert_eq!(cookbook.vault_root(), root.as_path());
}
