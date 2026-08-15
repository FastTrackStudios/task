//! File-backed [`IntakeService`] impl. Holds shared
//! `mealplan` stores so recipe / pantry nutrition can
//! resolve.
//!
//! CRUD is [`vault_entity::VaultEntityStore`]; the day-keyed upsert
//! (`for_day`) and the nutrition-resolving `log_*` helpers — the parts
//! specific to intake — live here.

use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use mealplan::MealNutrition;
use mealplan::cookbook::{CookbookService, Nutrition};
use mealplan::pantry::PantryService;
use uuid::Uuid;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::IntakeLogs;
use crate::model::{IntakeEntry, IntakeLog, IntakeSource, scale_nutrition};
use crate::service::{IntakeError, IntakeService};
use crate::write::default_intake_path;

vault_entity::entity_error_bridge!(IntakeError);

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: VaultEntityStore<IntakeLogs>,
    cookbook: mealplan::cookbook::Store,
    pantry: mealplan::pantry::Store,
}

impl Store {
    pub fn new(vault: Vault) -> Self {
        let root = vault.root.clone();
        let pantry = mealplan::pantry::Store::new(vault);
        let inner = VaultEntityStore::from_shared(pantry.shared());
        let cookbook = mealplan::cookbook::Store::new(root);
        Self {
            inner,
            cookbook,
            pantry,
        }
    }

    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        let root = inner.lock().expect("shared vault poisoned").root.clone();
        let cookbook = mealplan::cookbook::Store::new(root);
        let pantry = mealplan::pantry::Store::from_shared(inner.clone());
        Self {
            inner: VaultEntityStore::from_shared(inner),
            cookbook,
            pantry,
        }
    }

    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.shared()
    }
}

fn parse_date(s: &str) -> Result<NaiveDate, IntakeError> {
    s.parse()
        .map_err(|e| IntakeError::BadRequest(format!("date: {e}")))
}

fn slot_to_opt(slot: &str) -> Option<String> {
    if slot.trim().is_empty() {
        None
    } else {
        Some(slot.to_string())
    }
}

impl IntakeService for Store {
    fn list(&self) -> Result<Vec<IntakeLog>, IntakeError> {
        Ok(self.inner.list())
    }

    fn get(&self, id: &str) -> Result<IntakeLog, IntakeError> {
        self.inner.get(id).map_err(from_entity_error)
    }

    fn for_day(&self, date: &str) -> Result<IntakeLog, IntakeError> {
        let day = parse_date(date)?;
        if let Some(log) = self.inner.find(|l| l.date == day) {
            return Ok(log);
        }
        // No log for `date` — create one.
        let log = IntakeLog {
            path: String::new(),
            id: Uuid::nil(),
            name: format!("Intake {date}"),
            date: day,
            entries: crate::model::Entries::default(),
            target: crate::model::DailyTarget::default(),
            tags: crate::model::Tags::default(),
            date_created: None,
            date_modified: None,
            details: String::new(),
        };
        self.create(log)
    }

    fn create(&self, mut log: IntakeLog) -> Result<IntakeLog, IntakeError> {
        // The filename is the ISO date, not a slug of the name, so the
        // path is resolved here rather than by the shared store.
        if log.path.is_empty() {
            log.path = default_intake_path(log.date, None);
        }
        self.inner.create(log).map_err(from_entity_error)
    }

    fn update(&self, log: IntakeLog) -> Result<IntakeLog, IntakeError> {
        self.inner.update(log).map_err(from_entity_error)
    }

    fn delete(&self, id: &str) -> Result<(), IntakeError> {
        self.inner.delete(id).map_err(from_entity_error)
    }

    fn log_recipe(
        &self,
        date: &str,
        recipe_path: &str,
        servings: f64,
        slot: &str,
    ) -> Result<IntakeLog, IntakeError> {
        if servings < 0.0 {
            return Err(IntakeError::BadRequest(
                "servings must be non-negative".into(),
            ));
        }
        let recipe = self
            .cookbook
            .get(recipe_path)
            .map_err(|e| IntakeError::Mealplan(format!("recipe lookup: {e}")))?;
        // Recipe nutrition is computed at intake time from
        // pantry per-unit data (the cooklang file carries no
        // nutrition). Build a synthetic 1-serving meal and
        // delegate; multiply by the caller's `servings`.
        let items = self
            .pantry
            .list()
            .map_err(|e| IntakeError::Mealplan(e.to_string()))?;
        let synthetic_meal = mealplan::Meal {
            path: String::new(),
            id: Uuid::nil(),
            name: recipe.name.clone(),
            scheduled_for: chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
            slot: "snack".into(),
            servings: 1,
            recipe_paths: mealplan::model::StringList(vec![recipe_path.into()]),
            status: "planned".into(),
            pantry_deductions: mealplan::model::PantryDeductions::default(),
            tags: mealplan::model::StringList::default(),
            date_created: None,
            date_modified: None,
            details: String::new(),
        };
        let per_serving = synthetic_meal.nutrition_total(std::slice::from_ref(&recipe), &items);
        let nutrition = per_serving.as_ref().map(|n| scale_nutrition(n, servings));
        let entry = IntakeEntry {
            id: Uuid::new_v4(),
            source: IntakeSource::Recipe {
                path: recipe_path.into(),
            },
            name: recipe.name.clone(),
            qty: servings,
            unit: "serving".into(),
            time: None,
            slot: slot_to_opt(slot),
            nutrition,
            note: None,
        };
        self.log_entry(date, entry)
    }

    fn log_pantry(
        &self,
        date: &str,
        item_id: &str,
        qty: f64,
        slot: &str,
    ) -> Result<IntakeLog, IntakeError> {
        if qty < 0.0 {
            return Err(IntakeError::BadRequest("qty must be non-negative".into()));
        }
        let item = self
            .pantry
            .get(item_id)
            .map_err(|e| IntakeError::Mealplan(format!("pantry lookup: {e}")))?;
        let id = Uuid::parse_str(item_id)
            .map_err(|e| IntakeError::BadRequest(format!("item_id: {e}")))?;
        let nutrition = compute_pantry_nutrition(&item, qty);
        let unit = item.unit.clone();
        let entry = IntakeEntry {
            id: Uuid::new_v4(),
            source: IntakeSource::Pantry { id },
            name: item.name.clone(),
            qty,
            unit,
            time: None,
            slot: slot_to_opt(slot),
            nutrition,
            note: None,
        };
        self.log_entry(date, entry)
    }

    fn log_freeform(
        &self,
        date: &str,
        name: &str,
        nutrition: Nutrition,
        slot: &str,
    ) -> Result<IntakeLog, IntakeError> {
        let entry = IntakeEntry {
            id: Uuid::new_v4(),
            source: IntakeSource::Freeform,
            name: name.to_string(),
            qty: 1.0,
            unit: String::new(),
            time: None,
            slot: slot_to_opt(slot),
            nutrition: Some(nutrition),
            note: None,
        };
        self.log_entry(date, entry)
    }

    fn log_entry(&self, date: &str, mut entry: IntakeEntry) -> Result<IntakeLog, IntakeError> {
        if entry.id.is_nil() {
            entry.id = Uuid::new_v4();
        }
        let mut log = self.for_day(date)?;
        log.entries.push(entry);
        self.update(log)
    }
}

/// Pantry nutrition is per `nutrition_unit_qty` of
/// `nutrition_unit` (typically per 100g). Scaling needs:
///
///   scale = qty / nutrition_unit_qty
///
/// We don't know `nutrition_unit_qty` numerically — OFF
/// returns it as a string `"100g"`. Parse the leading
/// digits; default to `100.0` so per-100g labels (the
/// common case) Just Work.
fn compute_pantry_nutrition(item: &mealplan::pantry::PantryItem, qty: f64) -> Option<Nutrition> {
    let n = item.nutrition_per_unit.as_ref()?;
    let unit_qty = item
        .nutrition_unit
        .as_deref()
        .and_then(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect::<String>()
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(100.0);
    let scale = if unit_qty > 0.0 { qty / unit_qty } else { qty };
    Some(scale_nutrition(n, scale))
}
