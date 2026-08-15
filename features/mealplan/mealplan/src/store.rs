//! File-backed [`MealplanService`] impl. Holds a shared
//! `vault::Vault` mutex and a [`pantry::Store`] so
//! [`MealplanService::cook`] can deduct stock atomically
//! against the same vault snapshot.

use std::sync::{Arc, Mutex};

use cookbook::Store as CookbookStore;
use cookbook::{self, CookbookService};
use pantry::{PantryService, Store as PantryStore};
use uuid::Uuid;
use vault::Vault;

use crate::fulfillment::{self, CookReceipt, Fulfillment};
use crate::model::{Meal, MealNutrition, PantryDeduction};
use crate::parse::{looks_like_meal, parse_page};
use crate::scan::scan_vault;
use crate::service::{MealplanError, MealplanService};
use crate::write::{default_meal_path, serialize_meal};

/// File-backed mealplan store.
///
/// Must share its `vault::Vault` mutex with the pantry store
/// it holds — otherwise the deduct-on-cook flow would race
/// against pantry edits made through the other surface.
/// Construct via [`Store::new`] from a single `Vault` so the
/// shared mutex is set up correctly.
#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: Arc<Mutex<Vault>>,
    pantry: PantryStore,
    cookbook: CookbookStore,
}

impl Store {
    /// Build mealplan + pantry + cookbook stores around one
    /// shared vault mutex. Cookbook walks the same vault
    /// root for `.cook` files.
    #[must_use]
    pub fn new(vault: Vault) -> Self {
        let root = vault.root.clone();
        let pantry = PantryStore::new(vault);
        let inner = pantry.shared();
        let cookbook = CookbookStore::new(root);
        Self {
            inner,
            pantry,
            cookbook,
        }
    }

    /// Reuse a vault mutex already in use by another feature
    /// (locations / inventory). Cookbook builds a new
    /// path-backed store from the shared vault's root.
    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        let root = inner.lock().expect("shared vault poisoned").root.clone();
        let pantry = PantryStore::from_shared(inner.clone());
        let cookbook = CookbookStore::new(root);
        Self {
            inner,
            pantry,
            cookbook,
        }
    }

    /// Swap in a cookbook store rooted elsewhere. The default
    /// from [`Store::new`] walks the *vault* root, but on the
    /// standard org layout recipes live under the wiki root
    /// (`<org>/wiki/Knowledge/Cookbook/`) — pass the same
    /// store the `CookbookService` mounts so `can_cook` and
    /// cook-deductions resolve the same recipe paths.
    #[must_use]
    pub fn with_cookbook(mut self, cookbook: CookbookStore) -> Self {
        self.cookbook = cookbook;
        self
    }

    #[must_use]
    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.clone()
    }

    #[must_use]
    pub fn pantry(&self) -> &PantryStore {
        &self.pantry
    }

    #[must_use]
    pub fn cookbook(&self) -> &CookbookStore {
        &self.cookbook
    }

    /// Convenience: total nutrition for the meal with `id`,
    /// computed from cooklang recipe ingredients × pantry
    /// per-unit nutrition. See [`Meal::nutrition_total`].
    pub fn meal_nutrition(&self, id: &str) -> Result<Option<cookbook::Nutrition>, MealplanError> {
        let meal = self.get(id)?;
        let recipes = self
            .cookbook
            .list()
            .map_err(|e| MealplanError::Pantry(format!("cookbook list: {e}")))?;
        let items = self
            .pantry
            .list()
            .map_err(|e| MealplanError::Pantry(e.to_string()))?;
        Ok(meal.nutrition_total(&recipes, &items))
    }
}

fn map_io(e: impl std::fmt::Display) -> MealplanError {
    MealplanError::Io(e.to_string())
}

fn find_idx(vault: &Vault, id: Uuid) -> Option<usize> {
    vault
        .pages
        .iter()
        .position(|p| looks_like_meal(p) && parse_page(p).map(|m| m.id == id).unwrap_or(false))
}

impl MealplanService for Store {
    fn list(&self) -> Result<Vec<Meal>, MealplanError> {
        let guard = self.inner.lock().expect("mealplan store poisoned");
        Ok(scan_vault(&guard))
    }

    fn get(&self, id: &str) -> Result<Meal, MealplanError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| MealplanError::BadRequest(format!("id: {e}")))?;
        let guard = self.inner.lock().expect("mealplan store poisoned");
        for page in guard.pages.iter().filter(|p| looks_like_meal(p)) {
            if let Ok(m) = parse_page(page) {
                if m.id == uuid {
                    return Ok(m);
                }
            }
        }
        Err(MealplanError::NotFound(id.to_string()))
    }

    fn create(&self, mut meal: Meal) -> Result<Meal, MealplanError> {
        if meal.id.is_nil() {
            meal.id = Uuid::new_v4();
        }
        if meal.path.is_empty() {
            meal.path = default_meal_path(meal.scheduled_for, &meal.slot, Some(&meal.name), None);
        }
        let now = chrono::Utc::now();
        meal.date_created.get_or_insert(now);
        meal.date_modified = Some(now);
        let body = serialize_meal(&meal).map_err(map_io)?;
        let mut guard = self.inner.lock().expect("mealplan store poisoned");
        if guard.pages.iter().any(|p| p.rel_path == meal.path) {
            return Err(MealplanError::AlreadyExists(meal.path));
        }
        vault::create_page(&mut guard, &meal.path, body).map_err(map_io)?;
        Ok(meal)
    }

    fn update(&self, mut meal: Meal) -> Result<Meal, MealplanError> {
        let mut guard = self.inner.lock().expect("mealplan store poisoned");
        let idx = find_idx(&guard, meal.id)
            .ok_or_else(|| MealplanError::NotFound(meal.id.to_string()))?;
        meal.path = guard.pages[idx].rel_path.clone();
        meal.date_modified = Some(chrono::Utc::now());
        let body = serialize_meal(&meal).map_err(map_io)?;
        guard.pages[idx].raw = body;
        let path = meal.path.clone();
        vault::save_page(&mut guard, &path).map_err(map_io)?;
        Ok(meal)
    }

    fn rename(&self, id: &str, new_path: &str) -> Result<Meal, MealplanError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| MealplanError::BadRequest(format!("id: {e}")))?;
        let mut guard = self.inner.lock().expect("mealplan store poisoned");
        let idx = find_idx(&guard, uuid).ok_or_else(|| MealplanError::NotFound(id.to_string()))?;
        if guard.pages.iter().any(|p| p.rel_path == new_path) {
            return Err(MealplanError::AlreadyExists(new_path.to_string()));
        }
        let old_path = guard.pages[idx].rel_path.clone();
        let raw = guard.pages[idx].raw.clone();
        vault::delete_page(&mut guard, &old_path).map_err(map_io)?;
        vault::create_page(&mut guard, new_path, raw).map_err(map_io)?;
        let new_page = guard
            .pages
            .iter()
            .find(|p| p.rel_path == new_path)
            .ok_or_else(|| MealplanError::Io("rename: page missing post-write".into()))?;
        parse_page(new_page).map_err(|e| MealplanError::Io(e.to_string()))
    }

    fn delete(&self, id: &str) -> Result<(), MealplanError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| MealplanError::BadRequest(format!("id: {e}")))?;
        let mut guard = self.inner.lock().expect("mealplan store poisoned");
        let idx = find_idx(&guard, uuid).ok_or_else(|| MealplanError::NotFound(id.to_string()))?;
        let path = guard.pages[idx].rel_path.clone();
        vault::delete_page(&mut guard, &path).map_err(map_io)?;
        Ok(())
    }

    fn cook(&self, id: &str, deductions: Vec<PantryDeduction>) -> Result<Meal, MealplanError> {
        // Deduct first, then stamp the meal. If a deduct
        // fails we surface the pantry error and leave the
        // meal unchanged — caller retries with a corrected
        // ingredient list. Uses `consume_stock` so the
        // FIFO + auto-open + shelf-life-after-open math
        // applies; falls back to the legacy `qty` path
        // automatically when an item has no stock entries.
        for row in &deductions {
            self.pantry
                .consume_stock(&row.item_id.to_string(), row.qty)
                .map_err(|e| MealplanError::Pantry(e.to_string()))?;
        }
        let mut meal = self.get(id)?;
        meal.status = crate::model::Status::Cooked.as_str().to_string();
        meal.pantry_deductions = crate::model::PantryDeductions(deductions);
        self.update(meal)
    }

    fn skip(&self, id: &str) -> Result<Meal, MealplanError> {
        let mut meal = self.get(id)?;
        meal.status = crate::model::Status::Skipped.as_str().to_string();
        self.update(meal)
    }

    fn eat_out(&self, id: &str) -> Result<Meal, MealplanError> {
        let mut meal = self.get(id)?;
        meal.status = crate::model::Status::EatingOut.as_str().to_string();
        self.update(meal)
    }

    fn cook_recipe(&self, recipe_path: &str, servings: u32) -> Result<CookReceipt, MealplanError> {
        let recipe = self
            .cookbook
            .get(recipe_path)
            .map_err(|e| MealplanError::NotFound(format!("recipe {recipe_path}: {e}")))?;
        let pantry_items = self
            .pantry
            .list()
            .map_err(|e| MealplanError::Pantry(e.to_string()))?;
        let receipt = crate::fulfillment::plan_cook(&recipe, &pantry_items, servings);
        for line in &receipt.deducted {
            self.pantry
                .consume_stock(&line.item_id.to_string(), line.qty)
                .map_err(|e| MealplanError::Pantry(e.to_string()))?;
        }
        Ok(receipt)
    }

    fn can_cook(&self, recipe_path: &str, servings: u32) -> Result<Fulfillment, MealplanError> {
        let recipe = self
            .cookbook
            .get(recipe_path)
            .map_err(|e| MealplanError::NotFound(format!("recipe {recipe_path}: {e}")))?;
        let pantry_items = self
            .pantry
            .list()
            .map_err(|e| MealplanError::Pantry(e.to_string()))?;
        // Pull every recipe — nested-recipe resolution
        // needs them. Recipes are cheap to list (small N
        // in any realistic vault); revisit if this gets hot.
        let all_recipes = if recipe.nested_recipes.is_empty() {
            Vec::new()
        } else {
            self.cookbook
                .list()
                .map_err(|e| MealplanError::Pantry(format!("cookbook list: {e}")))?
        };
        Ok(if recipe.nested_recipes.is_empty() {
            fulfillment::check(&recipe, &pantry_items, servings)
        } else {
            fulfillment::check_nested(&recipe, &all_recipes, &pantry_items, servings)
        })
    }
}
