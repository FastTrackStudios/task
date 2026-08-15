//! Recipe-to-recipe references.
//!
//! A cookbook with shared components — a sauce made once and used by
//! four dishes — only works if `@@./sauce` actually resolves. Cooklang
//! hands the reference back as written (`./sauce`) while recipes are
//! indexed by vault path (`Cookbook/sauce.cook`), so these pin the
//! normalisation between the two, and pin that a referenced recipe is
//! counted exactly once.

use cookbook::CookbookService;
use mealplan::fulfillment;
use std::path::Path;
use tempfile::tempdir;

fn cookbook_with(root: &Path, files: &[(&str, &str)]) -> Vec<cookbook::Recipe> {
    std::fs::create_dir_all(root.join("Cookbook")).unwrap();
    for (name, src) in files {
        std::fs::write(root.join(format!("Cookbook/{name}.cook")), src).unwrap();
    }
    let store = cookbook::Store::new(root.to_path_buf());
    store.list().unwrap()
}

const SAUCE: &str =
    ">> title: Sauce\n>> servings: 4\n\nBlend @yogurt{200%g} and @hot sauce{100%g}.\n";

fn find<'a>(all: &'a [cookbook::Recipe], stem: &str) -> &'a cookbook::Recipe {
    all.iter()
        .find(|r| r.path.contains(stem))
        .unwrap_or_else(|| panic!("no recipe matching {stem}"))
}

#[test]
fn a_reference_pulls_in_the_referenced_recipes_ingredients() {
    let tmp = tempdir().unwrap();
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 4\n\nServe @rice{300%g} with @@./sauce{4}.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 4);
    let names: Vec<&str> = flat.missing.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"yogurt") && names.contains(&"hot sauce"),
        "the sauce's own ingredients should reach the shortage list, got {names:?}"
    );
}

#[test]
fn the_reference_itself_is_not_also_counted() {
    let tmp = tempdir().unwrap();
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 4\n\nServe @rice{300%g} with @@./sauce{4}.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 4);
    let names: Vec<&str> = flat.missing.iter().map(|s| s.name.as_str()).collect();
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("sauce")),
        "an expanded reference must not also appear as an opaque row, got {names:?}"
    );
}

#[test]
fn an_unresolved_reference_survives_as_an_ingredient() {
    let tmp = tempdir().unwrap();
    // No `sauce.cook` in the cookbook at all.
    let all = cookbook_with(
        tmp.path(),
        &[(
            "bowl",
            ">> title: Bowl\n>> servings: 4\n\nServe @rice{300%g} with @@./sauce{4}.\n",
        )],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 4);
    let names: Vec<&str> = flat.missing.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("sauce")),
        "a reference that can't be resolved should still tell you to get it, got {names:?}"
    );
}

#[test]
fn reference_quantity_scales_the_child() {
    let tmp = tempdir().unwrap();
    // Sauce serves 4 (200 g yogurt). Asking for 2 servings should pull
    // in half of it.
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 1\n\nServe @rice{100%g} with @@./sauce{2}.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 1);
    let yogurt = flat
        .missing
        .iter()
        .find(|s| s.name == "yogurt")
        .expect("yogurt shortage");
    assert!(
        (yogurt.need - 100.0).abs() < 0.01,
        "2 of the sauce's 4 servings is half its 200 g of yogurt, got {}",
        yogurt.need
    );
}

#[test]
fn a_bare_reference_resolves_by_name() {
    let tmp = tempdir().unwrap();
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 4\n\nServe @rice{300%g} with @@sauce{4}.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 4);
    let names: Vec<&str> = flat.missing.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"yogurt"),
        "`@@sauce` without a path should still find the recipe, got {names:?}"
    );
}

// ── Vault link form ─────────────────────────────────────────────────
//
// `[[Sauce]]{n}` is the spelling to prefer: the same syntax every other
// note in the vault uses, so recipes participate in the wiki graph
// rather than carrying a private path convention.

#[test]
fn a_wikilink_reference_pulls_the_recipe_in() {
    let tmp = tempdir().unwrap();
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 4\n\nServe @rice{300%g} with [[Sauce]]{4}.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 4);
    let names: Vec<&str> = flat.missing.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"yogurt") && names.contains(&"hot sauce"),
        "the linked recipe's ingredients should reach the list, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("sauce")),
        "and the link itself shouldn't also be counted, got {names:?}"
    );
}

/// The reason the braces are load-bearing. Recipes link to concepts,
/// techniques and each other in prose; if a bare wikilink counted as a
/// reference, "see also [[Sauce]]" would put its ingredients on your
/// shopping list.
#[test]
fn a_bare_wikilink_is_not_a_recipe_reference() {
    let tmp = tempdir().unwrap();
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 4\n\nServe @rice{300%g}. Good with [[Sauce]] too.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 4);
    let names: Vec<&str> = flat.missing.iter().map(|s| s.name.as_str()).collect();
    assert!(
        !names.contains(&"yogurt"),
        "a prose mention must not drag in ingredients, got {names:?}"
    );
}

#[test]
fn a_wikilink_without_a_quantity_means_one_whole_batch() {
    let tmp = tempdir().unwrap();
    // Sauce serves 4 on 200 g of yogurt.
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 1\n\nServe @rice{100%g} with [[Sauce]]{}.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 1);
    let yogurt = flat
        .missing
        .iter()
        .find(|s| s.name == "yogurt")
        .expect("yogurt shortage");
    assert!(
        (yogurt.need - 200.0).abs() < 0.01,
        "a batch is the whole 200 g, got {}",
        yogurt.need
    );
}

#[test]
fn a_wikilink_alias_still_resolves() {
    let tmp = tempdir().unwrap();
    let all = cookbook_with(
        tmp.path(),
        &[
            ("sauce", SAUCE),
            (
                "bowl",
                ">> title: Bowl\n>> servings: 4\n\nServe @rice{300%g} with [[Sauce|the good stuff]]{4}.\n",
            ),
        ],
    );

    let flat = fulfillment::check_nested(find(&all, "bowl"), &all, &[], 4);
    let names: Vec<&str> = flat.missing.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"yogurt"), "got {names:?}");
}
