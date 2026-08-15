//! The two-stage shopping run, and templates.
//!
//! The flow this covers is the one a person actually walks: build a
//! list, go round the kitchen ticking off what's already there, then
//! take what's left to the shop. Only the shop half touches the pantry
//! — finding a jar you already owned must not invent stock.
//!
//! Templates are the other half: a list you keep (staples, a recipe's
//! ingredients) and start fresh whenever you cook it again, without
//! retyping it or losing last week's ticks.

use std::path::Path;

use mealplan::shopping::{EntryStatus, ShoppingEntries, ShoppingEntry, ShoppingList};
use mealplan::{ShoppingService, ShoppingStore};
use tempfile::tempdir;
use uuid::Uuid;
use vault::Vault;

fn fresh_vault(root: &Path) -> Vault {
    std::fs::create_dir_all(root.join("shopping")).unwrap();
    std::fs::create_dir_all(root.join("Operations/Inventory/Pantry")).unwrap();
    Vault::open(root).expect("open vault")
}

fn entry(name: &str) -> ShoppingEntry {
    ShoppingEntry {
        id: Uuid::new_v4(),
        item_id: None,
        name: name.into(),
        qty: Some(1.0),
        unit: "each".into(),
        note: None,
        status: EntryStatus::Needed,
    }
}

fn list_named(name: &str, is_template: bool, names: &[&str]) -> ShoppingList {
    ShoppingList {
        path: String::new(),
        id: Uuid::nil(),
        name: name.into(),
        store_location_id: None,
        entries: ShoppingEntries(names.iter().map(|n| entry(n)).collect()),
        is_template,
        from_template: None,
        date_created: None,
        date_modified: None,
        details: String::new(),
    }
}

#[test]
fn kitchen_pass_then_shop_pass() {
    let tmp = tempdir().unwrap();
    let store = ShoppingStore::new(fresh_vault(tmp.path()));

    let list = store
        .create(list_named("Sunday run", false, &["oats", "milk", "honey"]))
        .unwrap();
    let id = list.id.to_string();
    let oats = list.entries[0].id.to_string();
    let milk = list.entries[1].id.to_string();

    // Round the kitchen: the oats are already in the cupboard.
    let list = store.mark_have(&id, &oats, true).unwrap();
    assert_eq!(list.entries[0].status, EntryStatus::Have);
    assert_eq!(
        list.entries.iter().filter(|e| !e.is_settled()).count(),
        2,
        "milk + honey still to buy"
    );

    // At the shop: milk goes in the basket.
    let list = store.mark_purchased(&id, &milk).unwrap();
    assert_eq!(list.entries[1].status, EntryStatus::Purchased);
    assert_eq!(
        list.entries.iter().filter(|e| !e.is_settled()).count(),
        1,
        "only honey outstanding"
    );

    // Have and Purchased both mean "got it", but stay distinguishable —
    // the UI shows them in different phases and only one restocks.
    assert_ne!(list.entries[0].status, list.entries[1].status);
}

#[test]
fn miscount_puts_a_row_back_on_the_list() {
    let tmp = tempdir().unwrap();
    let store = ShoppingStore::new(fresh_vault(tmp.path()));
    let list = store.create(list_named("Run", false, &["flour"])).unwrap();
    let id = list.id.to_string();
    let flour = list.entries[0].id.to_string();

    let list = store.mark_have(&id, &flour, true).unwrap();
    assert_eq!(list.entries[0].status, EntryStatus::Have);
    // Jar turned out to be empty.
    let list = store.mark_have(&id, &flour, false).unwrap();
    assert_eq!(list.entries[0].status, EntryStatus::Needed);
}

#[test]
fn reset_keeps_the_rows_unlike_clear() {
    let tmp = tempdir().unwrap();
    let store = ShoppingStore::new(fresh_vault(tmp.path()));
    let list = store
        .create(list_named("Staples", false, &["salt", "oil"]))
        .unwrap();
    let id = list.id.to_string();
    let salt = list.entries[0].id.to_string();
    store.mark_purchased(&id, &salt).unwrap();

    let reset = store.reset(&id).unwrap();
    assert_eq!(reset.entries.len(), 2, "rows survive a reset");
    assert!(
        reset
            .entries
            .iter()
            .all(|e| e.status == EntryStatus::Needed),
        "every row is outstanding again"
    );

    let cleared = store.clear(&id).unwrap();
    assert!(cleared.entries.is_empty(), "clear drops the rows");
}

#[test]
fn starting_from_a_template_leaves_the_template_alone() {
    let tmp = tempdir().unwrap();
    let store = ShoppingStore::new(fresh_vault(tmp.path()));

    let template = store
        .create(list_named("Weekly staples", true, &["oats", "milk"]))
        .unwrap();
    let template_id = template.id.to_string();

    let run = store
        .start_from_template(&template_id, "Run — 14 Aug")
        .unwrap();
    assert!(!run.is_template);
    assert_eq!(run.from_template, Some(template.id));
    assert_eq!(run.name, "Run — 14 Aug");
    assert_eq!(run.entries.len(), 2);
    assert!(run.entries.iter().all(|e| e.status == EntryStatus::Needed));

    // Fresh entry ids — otherwise two concurrent runs of one template
    // would collide on `mark_purchased`.
    let template_ids: Vec<Uuid> = template.entries.iter().map(|e| e.id).collect();
    assert!(
        run.entries.iter().all(|e| !template_ids.contains(&e.id)),
        "run rows get their own ids"
    );

    // Shopping the run must not tick the template off.
    let run_id = run.id.to_string();
    store
        .mark_purchased(&run_id, &run.entries[0].id.to_string())
        .unwrap();
    let template_after = store.get(&template_id).unwrap();
    assert!(
        template_after
            .entries
            .iter()
            .all(|e| e.status == EntryStatus::Needed),
        "the template is reusable next week"
    );
}

#[test]
fn a_finished_run_can_be_kept_as_a_template() {
    let tmp = tempdir().unwrap();
    let store = ShoppingStore::new(fresh_vault(tmp.path()));
    let run = store
        .create(list_named("This week", false, &["rice", "beans"]))
        .unwrap();
    let run_id = run.id.to_string();
    store
        .mark_purchased(&run_id, &run.entries[0].id.to_string())
        .unwrap();

    let template = store.save_as_template(&run_id, "Store cupboard").unwrap();
    assert!(template.is_template);
    assert_eq!(template.entries.len(), 2);
    assert!(
        template
            .entries
            .iter()
            .all(|e| e.status == EntryStatus::Needed),
        "a template starts clean regardless of the run it came from"
    );

    // Templates and runs live side by side; the UI separates them.
    let all = store.list().unwrap();
    assert_eq!(all.iter().filter(|l| l.is_template).count(), 1);
    assert_eq!(all.iter().filter(|l| !l.is_template).count(), 1);
}

#[test]
fn legacy_purchased_flag_still_reads() {
    let tmp = tempdir().unwrap();
    // A list written before the two-stage run existed.
    std::fs::create_dir_all(tmp.path().join("shopping")).unwrap();
    std::fs::write(
        tmp.path().join("shopping/old.md"),
        "---\ntype: shopping-list\nid: 488737d5-57bd-40d3-9fc0-246e13013082\n\
         name: Shopping\nentries:\n- id: 686bc73a-3ea2-4d74-8fd1-4530ecaa5b97\n  \
         name: milk\n  qty: 1.5\n  unit: cup\n  purchased: true\n---\n",
    )
    .unwrap();
    let store = ShoppingStore::new(fresh_vault(tmp.path()));
    let all = store.list().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(
        all[0].entries[0].status,
        EntryStatus::Purchased,
        "`purchased: true` keeps its tick under the new model"
    );
}

/// `add_recipe_ingredients` lists everything the dish needs, scaled to
/// the batch — the gather checklist, before any pantry opinion.
#[test]
fn recipe_ingredients_build_a_gather_list() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("Cookbook")).unwrap();
    std::fs::write(
        root.join("Cookbook/pasta.cook"),
        ">> title: Pasta\n>> servings: 2\n\nBoil @spaghetti{200%g} with @salt{1%pinch}.\n\n\
         Toss with @olive oil{3%tbsp} and @?parmesan{30%g}.\n",
    )
    .unwrap();
    let store = ShoppingStore::new(fresh_vault(root))
        .with_cookbook(cookbook::Store::new(root.to_path_buf()));

    let list = store.create(list_named("Gather", false, &[])).unwrap();
    let list = store
        .add_recipe_ingredients(&list.id.to_string(), "Cookbook/pasta.cook", 4)
        .unwrap();

    let names: Vec<&str> = list.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"spaghetti") && names.contains(&"olive oil"),
        "every required ingredient lands on the list, got {names:?}"
    );
    assert!(
        !names.contains(&"parmesan"),
        "an optional `@?` ingredient isn't something the dish needs"
    );

    // 2 servings → 4 doubles the quantities.
    let spaghetti = list
        .entries
        .iter()
        .find(|e| e.name == "spaghetti")
        .expect("spaghetti row");
    assert_eq!(spaghetti.qty, Some(400.0), "scaled to the batch");
    assert_eq!(spaghetti.unit, "g");
    assert!(
        list.entries.iter().all(|e| e.status == EntryStatus::Needed),
        "a gather list starts with everything outstanding — the kitchen \
         pass is the cook's job, not the pantry's guess"
    );
}
