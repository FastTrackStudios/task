//! What cooklang knows about quantities, and what we do with it.
//!
//! Both behaviours here were previously hand-rolled and wrong at the
//! edges: merging compared unit strings so `500 g` and `0.5 kg` stayed
//! apart, and scaling multiplied every quantity including the ones
//! cooklang had explicitly pinned.

use cookbook::CookbookService;
use mealplan::fulfillment;
use std::path::Path;
use tempfile::tempdir;

fn cookbook_with(root: &Path, files: &[(&str, &str)]) -> Vec<cookbook::Recipe> {
    std::fs::create_dir_all(root.join("Cookbook")).unwrap();
    for (name, src) in files {
        std::fs::write(root.join(format!("Cookbook/{name}.cook")), src).unwrap();
    }
    cookbook::Store::new(root.to_path_buf()).list().unwrap()
}

fn find<'a>(all: &'a [cookbook::Recipe], stem: &str) -> &'a cookbook::Recipe {
    all.iter().find(|r| r.path.contains(stem)).unwrap()
}

#[test]
fn the_same_ingredient_in_different_units_becomes_one_row() {
    let tmp = tempdir().unwrap();
    // The dough calls for grams, the topping for kilograms. A shopping
    // list that shows both is telling you to buy flour twice.
    let all = cookbook_with(
        tmp.path(),
        &[
            (
                "dough",
                ">> title: Dough\n>> servings: 1\n\nMix @flour{500%g} with water.\n",
            ),
            (
                "pie",
                ">> title: Pie\n>> servings: 1\n\nMake [[Dough]]{1}, then dust with @flour{0.5%kg}.\n",
            ),
        ],
    );

    let f = fulfillment::check_nested(find(&all, "pie"), &all, &[], 1);
    let flour: Vec<_> = f
        .missing
        .iter()
        .filter(|s| s.name.eq_ignore_ascii_case("flour"))
        .collect();
    assert_eq!(flour.len(), 1, "one flour row, got {flour:?}");
    // The row lands in whichever unit was mentioned first, so compare
    // in grams rather than assuming which that was.
    let grams = cookbook::convert(flour[0].need, &flour[0].unit, "g")
        .expect("a flour row should be in a mass unit");
    assert!(
        (grams - 1000.0).abs() < 0.01,
        "500 g + 0.5 kg is 1000 g, got {} {} ({grams} g)",
        flour[0].need,
        flour[0].unit
    );
}

#[test]
fn units_that_dont_reconcile_stay_separate() {
    let tmp = tempdir().unwrap();
    // Grams of garlic and cloves of garlic can't be added without
    // knowing how big a clove is. Two rows is the honest answer.
    let all = cookbook_with(
        tmp.path(),
        &[(
            "x",
            ">> title: X\n>> servings: 1\n\nAdd @garlic{2%clove}.\n\nThen @garlic{30%g}.\n",
        )],
    );
    let f = fulfillment::check_nested(find(&all, "x"), &all, &[], 1);
    let garlic = f
        .missing
        .iter()
        .filter(|s| s.name.eq_ignore_ascii_case("garlic"))
        .count();
    assert_eq!(garlic, 2, "cloves and grams shouldn't be silently added");
}

#[test]
fn a_pinned_quantity_is_marked_unscalable() {
    let tmp = tempdir().unwrap();
    // `=` pins a quantity: you season the pan once, not once per head.
    let all = cookbook_with(
        tmp.path(),
        &[(
            "x",
            ">> title: X\n>> servings: 2\n\nSeason with @salt{=1%tsp} and @flour{100%g}.\n",
        )],
    );
    let r = find(&all, "x");
    let salt = r.ingredients.iter().find(|i| i.name == "salt").unwrap();
    let flour = r.ingredients.iter().find(|i| i.name == "flour").unwrap();
    assert!(!salt.scalable, "`=1%tsp` is pinned");
    assert!(flour.scalable, "an ordinary quantity scales");
}

#[test]
fn a_range_keeps_both_ends() {
    let tmp = tempdir().unwrap();
    let all = cookbook_with(
        tmp.path(),
        &[("x", ">> title: X\n>> servings: 1\n\nAdd @oil{1-2%tbsp}.\n")],
    );
    let r = find(&all, "x");
    let oil = r.ingredients.iter().find(|i| i.name == "oil").unwrap();
    assert_eq!(oil.qty, Some(1.0), "low end");
    assert_eq!(
        oil.qty_max,
        Some(2.0),
        "high end — doubling gives 2–4, not 3"
    );
}
