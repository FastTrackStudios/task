//! Mealplan — the cookbook, the pantry, and everything cooking with
//! them.
//!
//! Six screens over one domain: the **plan** (this week's meals and
//! the recipe list), **cook mode** (a recipe as a working surface with
//! its own timers), **read** (the whole recipe on one page), **edit**
//! (its cooklang source), and the two-pass **shopping** run — kitchen
//! first, then store.
//!
//! ## Recipes are files
//!
//! A recipe is a `.cook` file in the vault, not a row in a table Task
//! keeps for this app. That is the trade the plugin design turns on:
//! sync, sharing, version history and readability by anything come for
//! free, and the app stores nothing only it can read. Everything here
//! is a view over those files.
//!
//! Mounted by `task-plugin-mealplan`.

use architect::Id;
use task_stores::run_create;
use task_ui_core::feeds;
use uuid::Uuid;

/// This app's id in Task's catalog, and the first segment of every
/// link it writes to itself.
pub const APP_ID: &str = "mealplan";

// ─────────────────────────────────────────────────────────────────────
// Service calls
// ─────────────────────────────────────────────────────────────────────

feeds! {
    cookbook_proto::CookbookServiceClient {
        /// Every recipe in the org's cookbook (`<wiki>/Cookbook/*.cook`),
        /// in the order the backend lists them.
        fetch_recipes() -> Vec<cookbook_proto::Recipe>
            = list() as "list recipes";

        /// Create one recipe from a caller-built draft (see
        /// `self::draft_recipe` — identity is the vault-relative `path`; the
        /// backend parses the cooklang `source`). Returns the persisted recipe.
        create_recipe(recipe: cookbook_proto::Recipe) -> cookbook_proto::Recipe
            = create(recipe) as "create recipe";

        /// Import a recipe from a web URL — the server fetches the page,
        /// extracts the recipe, and synthesizes a cooklang `.cook` draft (not
        /// yet saved). Returns the parsed draft for review.
        import_recipe(url: String) -> cookbook_proto::Recipe
            = import(url) as "import recipe";

        /// Raw bytes of one recipe image, addressed by the wiki-relative
        /// path carried on `Recipe::images`. Served over the org's RPC
        /// rather than a public HTTP route, so it inherits the same
        /// permit gate as the recipes themselves.
        fetch_recipe_image(path: String) -> Vec<u8>
            = image(path) as "recipe image";

        /// Save edits to a recipe's `.cook` source. The server writes the
        /// source verbatim then re-parses, so the returned recipe carries fresh
        /// structured steps / ingredients / timers.
        update_recipe(recipe: cookbook_proto::Recipe) -> cookbook_proto::Recipe
            = update(recipe) as "update recipe";
    }

    pantry_proto::PantryServiceClient {
        /// Every pantry item in the org's vault (food-on-hand pages), in
        /// the order the backend lists them.
        fetch_pantry() -> Vec<pantry_proto::PantryItem>
            = list() as "list pantry";

        /// Create one pantry item from a caller-built draft (see
        /// `self::draft_pantry_item` — the backend assigns the real `id` and
        /// vault `path`). Returns the persisted item.
        create_pantry_item(item: pantry_proto::PantryItem) -> pantry_proto::PantryItem
            = create(item) as "create pantry item";
    }

    mealplan_proto::MealplanServiceClient {
        /// Cook a recipe directly: the server computes the pantry deductions
        /// for `servings` and consumes them from stock, returning a receipt of
        /// what was deducted (matched + convertible + in-stock ingredients) and
        /// what was skipped (with the reason).
        cook_recipe(recipe_path: String, servings: u32) -> mealplan_proto::CookReceipt
            = cook_recipe(recipe_path, servings) as "cook recipe";

        /// "Can I cook this right now?" — the server checks the recipe (and any
        /// nested recipes) against current pantry stock for `servings` and
        /// returns the full `Fulfillment`: whether it's cookable, the
        /// have/need partition, and the per-shortage substitution suggestions.
        /// All derivation is server-side — this is a thin client call.
        can_cook(recipe_path: String, servings: u32) -> mealplan_proto::Fulfillment
            = can_cook(recipe_path, servings) as "can cook";

        /// Every planned meal in the org's vault, in the order the
        /// backend lists them.
        fetch_meal_plans() -> Vec<mealplan_proto::Meal>
            = list() as "list meal plans";
    }

    mealplan_proto::ShoppingServiceClient {
        /// Every shopping list in the org's vault — live runs and the
        /// reusable templates alongside them (tell them apart by
        /// `is_template`).
        fetch_shopping_lists() -> Vec<mealplan_proto::ShoppingList>
            = list() as "list shopping lists";

        /// Create a list from a caller-built draft; the backend assigns
        /// the vault `path` and stamps the dates.
        create_shopping_list(list: mealplan_proto::ShoppingList) -> mealplan_proto::ShoppingList
            = create(list) as "create shopping list";

        /// Save edits (renames, hand-added rows) verbatim.
        update_shopping_list(list: mealplan_proto::ShoppingList) -> mealplan_proto::ShoppingList
            = update(list) as "update shopping list";

        /// First pass: tick a row off because it's already in the
        /// kitchen. Deliberately no pantry write — pass `have = false`
        /// to put it back on the list.
        mark_have(list_id: String, entry_id: String, have: bool) -> mealplan_proto::ShoppingList
            = mark_have(list_id, entry_id, have) as "mark have";

        /// Second pass: bought it. Restocks the pantry when the row is
        /// linked to a pantry item and carries a quantity.
        mark_purchased(list_id: String, entry_id: String) -> mealplan_proto::ShoppingList
            = mark_purchased(list_id, entry_id) as "mark purchased";

        /// Put every row back to `needed` — re-run the same list next
        /// week without retyping it. Keeps the rows (unlike `clear`).
        reset_shopping_list(id: String) -> mealplan_proto::ShoppingList
            = reset(id) as "reset shopping list";

        /// Start a fresh run from a template; the template is untouched.
        start_from_template(template_id: String, name: String) -> mealplan_proto::ShoppingList
            = start_from_template(template_id, name) as "start from template";

        /// Keep this list's rows as a reusable template.
        save_as_template(list_id: String, name: String) -> mealplan_proto::ShoppingList
            = save_as_template(list_id, name) as "save as template";

        /// Add everything a recipe needs that the pantry can't cover at
        /// `servings` — the "what do I need to buy for this meal" button.
        add_missing_for_recipe(list_id: String, recipe_path: String, servings: u32)
            -> mealplan_proto::ShoppingList
            = add_missing_for_recipe(list_id, recipe_path, servings) as "add missing for recipe";

        /// Add everything a recipe calls for at `servings`, whatever the
        /// pantry says — the gather checklist, where the kitchen pass is
        /// an actual look at an actual shelf rather than a stock guess.
        add_recipe_ingredients(list_id: String, recipe_path: String, servings: u32)
            -> mealplan_proto::ShoppingList
            = add_recipe_ingredients(list_id, recipe_path, servings) as "add recipe ingredients";

        /// Add every pantry item at or below its reorder minimum.
        add_low_stock(list_id: String) -> mealplan_proto::ShoppingList
            = add_low_stock(list_id) as "add low stock";
    }
}

// ─────────────────────────────────────────────────────────────────────
// Stores
// ─────────────────────────────────────────────────────────────────────

task_stores::stores! {
    RecipeStore: cookbook_proto::Recipe {
        provide: provide_recipe_store,
        handle: use_recipe_store,
        list: use_recipe_list -> String = fetch_recipes,
        mutations: RecipeMutations via use_recipe_mutations,
    }

    PantryStore: pantry_proto::PantryItem {
        provide: provide_pantry_store,
        handle: use_pantry_store,
        list: use_pantry_list -> Uuid = fetch_pantry,
        mutations: PantryMutations via use_pantry_mutations,
    }
}

/// Unsaved placeholder row for an optimistic recipe insert. Identity is
/// the vault-relative `path`; the store keys the draft by a typed
/// `Id::Temp` until the server's row reconciles in, so no magic
/// `__pending__` path sentinel is needed.
pub fn draft_recipe(name: String) -> cookbook_proto::Recipe {
    cookbook_proto::Recipe {
        path: format!("Cookbook/{name}.cook"),
        source: format!(">> title: {name}\n"),
        name,
        description: None,
        course: None,
        cuisine: None,
        prep_minutes: None,
        cook_minutes: None,
        servings: None,
        ingredients: cookbook_proto::Ingredients::default(),
        steps: cookbook_proto::StringList::default(),
        cook_steps: cookbook_proto::CookSteps::default(),
        cookware: cookbook_proto::StringList::default(),
        nested_recipes: cookbook_proto::StringList::default(),
        tags: cookbook_proto::StringList::default(),
        source_url: None,
        date_modified: None,
        // Found on disk by the server; a new draft has none yet.
        images: cookbook_proto::RecipeImages::default(),
    }
}

impl RecipeMutations {
    pub fn create(&self, slug: String, draft: cookbook_proto::Recipe) {
        run_create(self.write, self.store, draft, move |recipe| async move {
            self::create_recipe(&slug, recipe).await
        });
    }

    /// Save an edited recipe (keyed by vault `path`): patch the store
    /// optimistically, write through, and reconcile the server's
    /// re-parsed row (fresh steps / timers) back in — or roll back +
    /// notify on failure.
    pub fn update(&self, slug: String, recipe: cookbook_proto::Recipe) {
        let key = recipe.path.clone();
        let row = recipe.clone();
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(key), move |r| *r = row),
            move || async move { self::update_recipe(&slug, recipe).await.map(Some) },
        );
    }
}

/// Unsaved placeholder row for an optimistic pantry insert.
pub fn draft_pantry_item(name: String, qty: Option<f64>, unit: String) -> pantry_proto::PantryItem {
    pantry_proto::PantryItem {
        path: String::new(),
        id: Uuid::nil(),
        name,
        category: "food".to_owned(),
        location_id: None,
        condition: "good".to_owned(),
        status: "stored".to_owned(),
        tags: pantry_proto::StringList(vec!["item".into(), "pantry".into()]),
        date_created: None,
        date_modified: None,
        food_category: String::new(),
        qty,
        unit,
        purchase_unit: None,
        purchase_to_stock_factor: None,
        expiry: None,
        opened: false,
        opened_date: None,
        brand: None,
        nutrition_per_unit: None,
        nutrition_unit: None,
        minimum: None,
        default_best_before_days: None,
        default_best_before_days_after_open: None,
        default_best_before_days_after_freezing: None,
        default_best_before_days_after_thawing: None,
        due_type: "best-before".to_owned(),
        stock_entries: pantry_proto::StockEntries::default(),
        substitutes: pantry_proto::Substitutions::default(),
        barcodes: pantry_proto::StringList::default(),
        image_url: None,
        details: String::new(),
    }
}

impl PantryMutations {
    pub fn create(&self, slug: String, draft: pantry_proto::PantryItem) {
        run_create(self.write, self.store, draft, move |item| async move {
            create_pantry_item(&slug, item).await
        });
    }
}

pub mod cook;
pub mod edit;
pub mod plan;
pub mod read;
pub mod shopping;
pub mod week;

pub use cook::RecipeCookView;
pub use edit::EditRecipeView;
pub use plan::MealplanView;
pub use read::RecipeReadView;
pub use shopping::ShoppingView;
