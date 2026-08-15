//! Shopping lists — modeled on grocy's `shopping_lists`
//! + `shopping_list` (multi-list with per-list entries).
//!
//! Each list is a markdown page (`type: shopping-list`)
//! under `<vault>/shopping/`. Entries reference pantry
//! items by `item_id` when known so "mark purchased" can
//! call `pantry::add_stock` automatically; free-text rows
//! (one-off purchases) are supported too.
//!
//! The auto-populate methods (`add_missing_for_meal`,
//! `add_low_stock`, `add_expired_or_overdue`) mirror
//! grocy's `/stock/shoppinglist/add-*` API endpoints.

use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, Utc};
use cookbook::CookbookService;
use pantry::{PantryService, Store as PantryStore};
use uuid::Uuid;
use vault::{Vault, VaultPage};

use crate::fulfillment::{Fulfillment, Shortage, ShortageReason};

// ── Model + service (re-exported from the wasm-clean proto) ──
//
// The model and the `ShoppingService` trait live in
// `mealplan_proto::shopping` so the web UI can bind the client
// directly; this crate keeps the parse / serialize / vault-backed
// `Store` side.
pub use mealplan_proto::shopping::{
    EntryStatus, ShoppingEntries, ShoppingEntry, ShoppingError, ShoppingList, ShoppingService,
    ShoppingServiceRpc,
};

#[cfg(feature = "vox")]
pub use mealplan_proto::shopping::{
    Service, ShoppingServiceClient, ShoppingServiceRpcDispatcher, layer, serve,
    shopping_service_rpc_service_descriptor,
};

// ── Parse / write ────────────────────────────────────────────

fn split_frontmatter(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

#[must_use]
pub fn looks_like_shopping_list(page: &VaultPage) -> bool {
    let Some((fm, _)) = split_frontmatter(&page.raw) else {
        return false;
    };
    let Ok(map) = serde_yaml::from_str::<serde_yaml::Mapping>(fm) else {
        return false;
    };
    map.get("type").and_then(|v| v.as_str()) == Some("shopping-list")
}

fn parse_page(page: &VaultPage) -> Option<ShoppingList> {
    let (fm, body) = split_frontmatter(&page.raw)?;
    let map: serde_yaml::Mapping = serde_yaml::from_str(fm).ok()?;
    let take_str = |k: &str| {
        map.get(k).and_then(|v| match v {
            serde_yaml::Value::String(s) => Some(s.clone()),
            serde_yaml::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let id = take_str("id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));
    let name = take_str("name").unwrap_or_else(|| page.basename.clone());
    let store_location_id = take_str("storeLocationId").and_then(|s| Uuid::parse_str(&s).ok());
    let entries: Vec<ShoppingEntry> = map
        .get("entries")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|row| {
                    let m = row.as_mapping()?;
                    let entry_id = m
                        .get("id")
                        .and_then(|v| v.as_str())
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .unwrap_or_else(Uuid::new_v4);
                    let item_id = m
                        .get("itemId")
                        .and_then(|v| v.as_str())
                        .and_then(|s| Uuid::parse_str(s).ok());
                    let name = m.get("name").and_then(|v| v.as_str())?.to_string();
                    let qty = m.get("qty").and_then(serde_yaml::Value::as_f64);
                    let unit = m
                        .get("unit")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let note = m
                        .get("note")
                        .and_then(|v| v.as_str())
                        .map(std::string::ToString::to_string);
                    // `status` is authoritative; `purchased: true` is
                    // the pre-two-stage spelling and still parses so
                    // lists written before templates keep their ticks.
                    let status = m
                        .get("status")
                        .and_then(|v| v.as_str())
                        .and_then(EntryStatus::from_str)
                        .or_else(|| {
                            m.get("purchased")
                                .and_then(serde_yaml::Value::as_bool)
                                .and_then(|p| p.then_some(EntryStatus::Purchased))
                        })
                        .unwrap_or_default();
                    Some(ShoppingEntry {
                        id: entry_id,
                        item_id,
                        name,
                        qty,
                        unit,
                        note,
                        status,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let date_created = take_str("dateCreated").and_then(|s| s.parse().ok());
    let date_modified = take_str("dateModified").and_then(|s| s.parse().ok());
    let is_template = map
        .get("isTemplate")
        .and_then(serde_yaml::Value::as_bool)
        .unwrap_or(false);
    let from_template = take_str("fromTemplate").and_then(|s| Uuid::parse_str(&s).ok());

    Some(ShoppingList {
        path: page.rel_path.clone(),
        id,
        name,
        store_location_id,
        entries: ShoppingEntries(entries),
        is_template,
        from_template,
        date_created,
        date_modified,
        details: body.to_string(),
    })
}

fn serialize(list: &ShoppingList) -> Result<String, ShoppingError> {
    let mut wrapper = serde_yaml::Mapping::new();
    wrapper.insert("type".into(), "shopping-list".into());
    let body = serde_yaml::to_value(list).map_err(|e| ShoppingError::Io(e.to_string()))?;
    if let serde_yaml::Value::Mapping(m) = body {
        for (k, v) in m {
            wrapper.insert(k, v);
        }
    }
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(wrapper))
        .map_err(|e| ShoppingError::Io(e.to_string()))?;
    let details = if list.details.is_empty() {
        String::new()
    } else if list.details.starts_with('\n') {
        list.details.clone()
    } else {
        format!("\n{}", list.details)
    };
    Ok(format!("---\n{yaml}---\n{details}"))
}

fn default_path(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                slug.push(lc);
            }
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("shopping-list");
    }
    format!("shopping/{slug}.md")
}

// ── Store ────────────────────────────────────────────────────

/// File-backed [`ShoppingService`] impl. Carries a
/// `pantry::Store` so auto-populate (low-stock,
/// expired/overdue) and `mark_purchased` can drive the
/// pantry directly. The mealplan store is *not* held —
/// `add_missing_for_recipe` accepts a recipe id + servings
/// and runs fulfillment itself.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Vault>>,
    pantry: PantryStore,
    cookbook: cookbook::Store,
}

impl Store {
    #[must_use]
    pub fn new(vault: Vault) -> Self {
        let root = vault.root.clone();
        let pantry = PantryStore::new(vault);
        let inner = pantry.shared();
        let cookbook = cookbook::Store::new(root);
        Self {
            inner,
            pantry,
            cookbook,
        }
    }

    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        let root = inner.lock().expect("shared vault poisoned").root.clone();
        let pantry = PantryStore::from_shared(inner.clone());
        let cookbook = cookbook::Store::new(root);
        Self {
            inner,
            pantry,
            cookbook,
        }
    }

    /// Swap in a cookbook store rooted elsewhere. The default
    /// walks the *vault* root, but on the standard org layout
    /// recipes live under the wiki root
    /// (`<org>/wiki/Knowledge/Cookbook/`) — pass the same
    /// store the `CookbookService` mounts so
    /// `add_missing_for_recipe` resolves the same recipe
    /// paths.
    #[must_use]
    pub fn with_cookbook(mut self, cookbook: cookbook::Store) -> Self {
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
}

fn map_io(e: impl std::fmt::Display) -> ShoppingError {
    ShoppingError::Io(e.to_string())
}

fn find_idx(vault: &Vault, id: Uuid) -> Option<usize> {
    vault
        .pages
        .iter()
        .position(|p| looks_like_shopping_list(p) && parse_page(p).is_some_and(|l| l.id == id))
}

fn push_or_merge(list: &mut ShoppingList, entry: ShoppingEntry) {
    // Merge same (item_id, unit) rows by summing qty —
    // mirrors grocy's "add to existing if present".
    if let Some(existing) = list.entries.iter_mut().find(|e| {
        !e.is_settled()
            && e.unit.eq_ignore_ascii_case(&entry.unit)
            && match (e.item_id, entry.item_id) {
                (Some(a), Some(b)) => a == b,
                (None, None) => e.name.eq_ignore_ascii_case(&entry.name),
                _ => false,
            }
    }) {
        let base = existing.qty.unwrap_or(0.0);
        let add = entry.qty.unwrap_or(0.0);
        if base > 0.0 || add > 0.0 {
            existing.qty = Some(base + add);
        }
        return;
    }
    list.entries.push(entry);
}

impl architect::HasDispatcher for Store {
    type Dispatcher = architect::dispatch::TokioBlockingDispatcher;
    fn dispatcher(&self) -> Self::Dispatcher {
        architect::dispatch::TokioBlockingDispatcher
    }
}

impl ShoppingService for Store {
    fn list(&self) -> Result<Vec<ShoppingList>, ShoppingError> {
        let guard = self.inner.lock().expect("shopping store poisoned");
        Ok(guard
            .pages
            .iter()
            .filter(|p| looks_like_shopping_list(p))
            .filter_map(parse_page)
            .collect())
    }

    fn get(&self, id: &str) -> Result<ShoppingList, ShoppingError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| ShoppingError::BadRequest(format!("id: {e}")))?;
        let guard = self.inner.lock().expect("shopping store poisoned");
        for page in guard.pages.iter().filter(|p| looks_like_shopping_list(p)) {
            if let Some(l) = parse_page(page) {
                if l.id == uuid {
                    return Ok(l);
                }
            }
        }
        Err(ShoppingError::NotFound(id.to_string()))
    }

    fn create(&self, mut list: ShoppingList) -> Result<ShoppingList, ShoppingError> {
        if list.id.is_nil() {
            list.id = Uuid::new_v4();
        }
        if list.path.is_empty() {
            list.path = default_path(&list.name);
        }
        let now = Utc::now();
        list.date_created.get_or_insert(now);
        list.date_modified = Some(now);
        let body = serialize(&list)?;
        let mut guard = self.inner.lock().expect("shopping store poisoned");
        if guard.pages.iter().any(|p| p.rel_path == list.path) {
            return Err(ShoppingError::AlreadyExists(list.path));
        }
        vault::create_page(&mut guard, &list.path, body).map_err(map_io)?;
        Ok(list)
    }

    fn update(&self, mut list: ShoppingList) -> Result<ShoppingList, ShoppingError> {
        let mut guard = self.inner.lock().expect("shopping store poisoned");
        let idx = find_idx(&guard, list.id)
            .ok_or_else(|| ShoppingError::NotFound(list.id.to_string()))?;
        list.path = guard.pages[idx].rel_path.clone();
        list.date_modified = Some(Utc::now());
        let body = serialize(&list)?;
        guard.pages[idx].raw = body;
        let path = list.path.clone();
        vault::save_page(&mut guard, &path).map_err(map_io)?;
        Ok(list)
    }

    fn delete(&self, id: &str) -> Result<(), ShoppingError> {
        let uuid =
            Uuid::parse_str(id).map_err(|e| ShoppingError::BadRequest(format!("id: {e}")))?;
        let mut guard = self.inner.lock().expect("shopping store poisoned");
        let idx = find_idx(&guard, uuid).ok_or_else(|| ShoppingError::NotFound(id.to_string()))?;
        let path = guard.pages[idx].rel_path.clone();
        vault::delete_page(&mut guard, &path).map_err(map_io)?;
        Ok(())
    }

    fn add_missing_for_recipe(
        &self,
        list_id: &str,
        recipe_path: &str,
        servings: u32,
    ) -> Result<ShoppingList, ShoppingError> {
        let recipe = self
            .cookbook
            .get(recipe_path)
            .map_err(|e| ShoppingError::NotFound(format!("recipe {recipe_path}: {e}")))?;
        let pantry_items = self
            .pantry
            .list()
            .map_err(|e| ShoppingError::Pantry(e.to_string()))?;
        let f: Fulfillment = if recipe.nested_recipes.is_empty() {
            crate::fulfillment::check(&recipe, &pantry_items, servings)
        } else {
            let all = self
                .cookbook
                .list()
                .map_err(|e| ShoppingError::Pantry(format!("cookbook list: {e}")))?;
            crate::fulfillment::check_nested(&recipe, &all, &pantry_items, servings)
        };

        let mut list = self.get(list_id)?;
        for short in f.missing.into_iter().filter(|s| {
            matches!(
                s.reason,
                ShortageReason::NotInPantry | ShortageReason::InsufficientQty
            )
        }) {
            push_or_merge(&mut list, shortage_to_entry(short));
        }
        self.update(list)
    }

    fn add_recipe_ingredients(
        &self,
        list_id: &str,
        recipe_path: &str,
        servings: u32,
    ) -> Result<ShoppingList, ShoppingError> {
        let recipe = self
            .cookbook
            .get(recipe_path)
            .map_err(|e| ShoppingError::NotFound(format!("recipe {recipe_path}: {e}")))?;
        // Scale to the batch being cooked, the same way cook mode's
        // gather list does. A recipe with no declared yield scales 1:1.
        let base = recipe.servings.unwrap_or(1).max(1);
        let factor = f64::from(servings.max(1)) / f64::from(base);

        let mut list = self.get(list_id)?;
        for ing in recipe.ingredients.iter() {
            // Optional extras aren't part of "what this dish needs".
            if ing.optional {
                continue;
            }
            push_or_merge(
                &mut list,
                ShoppingEntry {
                    id: Uuid::new_v4(),
                    item_id: None,
                    name: ing.name.clone(),
                    qty: ing.qty.map(|q| q * factor),
                    unit: ing.unit.clone(),
                    note: Some(format!("for {}", recipe.name)),
                    status: EntryStatus::Needed,
                },
            );
        }
        self.update(list)
    }

    fn add_low_stock(&self, list_id: &str) -> Result<ShoppingList, ShoppingError> {
        let mut list = self.get(list_id)?;
        let items = self
            .pantry
            .list()
            .map_err(|e| ShoppingError::Pantry(e.to_string()))?;
        for item in items.into_iter().filter(pantry::PantryItem::is_low) {
            let need = item
                .minimum
                .zip(item.stock_total())
                .map_or(0.0, |(min, have)| (min - have).max(0.0));
            push_or_merge(
                &mut list,
                ShoppingEntry {
                    id: Uuid::new_v4(),
                    item_id: Some(item.id),
                    name: item.name.clone(),
                    qty: if need > 0.0 { Some(need) } else { None },
                    unit: item.unit.clone(),
                    note: Some("low stock".into()),
                    status: EntryStatus::Needed,
                },
            );
        }
        self.update(list)
    }

    fn add_expired_or_overdue(
        &self,
        list_id: &str,
        today: NaiveDate,
    ) -> Result<ShoppingList, ShoppingError> {
        let mut list = self.get(list_id)?;
        let items = self
            .pantry
            .list()
            .map_err(|e| ShoppingError::Pantry(e.to_string()))?;
        for item in items {
            if item.stock_entries.iter().any(|e| e.is_expired(today)) || item.is_expired(today) {
                push_or_merge(
                    &mut list,
                    ShoppingEntry {
                        id: Uuid::new_v4(),
                        item_id: Some(item.id),
                        name: item.name.clone(),
                        qty: None,
                        unit: item.unit.clone(),
                        note: Some("replace — expired".into()),
                        status: EntryStatus::Needed,
                    },
                );
            }
        }
        self.update(list)
    }

    fn clear(&self, id: &str) -> Result<ShoppingList, ShoppingError> {
        let mut list = self.get(id)?;
        list.entries.clear();
        self.update(list)
    }

    fn mark_purchased(&self, list_id: &str, entry_id: &str) -> Result<ShoppingList, ShoppingError> {
        let entry_uuid = Uuid::parse_str(entry_id)
            .map_err(|e| ShoppingError::BadRequest(format!("entry_id: {e}")))?;
        let mut list = self.get(list_id)?;
        let entry = list
            .entries
            .iter_mut()
            .find(|e| e.id == entry_uuid)
            .ok_or_else(|| ShoppingError::NotFound(format!("entry: {entry_id}")))?;
        entry.status = EntryStatus::Purchased;
        let pantry_item_id = entry.item_id;
        let qty = entry.qty;
        let unit = entry.unit.clone();

        // Optional pantry-add. Only fire when the entry was
        // linked to a known pantry item and the qty is
        // explicit — free-text rows without qty just flip
        // purchased + leave pantry alone.
        if let (Some(item_id), Some(q)) = (pantry_item_id, qty) {
            self.pantry
                .add_stock(
                    &item_id.to_string(),
                    pantry::StockEntry {
                        id: Uuid::new_v4(),
                        qty: q,
                        purchased_date: Utc::now().date_naive(),
                        best_before: None,
                        opened: false,
                        opened_date: None,
                        price: None,
                        location_id: None,
                        note: if unit.is_empty() {
                            None
                        } else {
                            Some(format!("shopping list ({unit})"))
                        },
                    },
                )
                .map_err(|e| ShoppingError::Pantry(e.to_string()))?;
        }
        self.update(list)
    }

    fn mark_have(
        &self,
        list_id: &str,
        entry_id: &str,
        have: bool,
    ) -> Result<ShoppingList, ShoppingError> {
        let entry_uuid = Uuid::parse_str(entry_id)
            .map_err(|e| ShoppingError::BadRequest(format!("entry_id: {e}")))?;
        let mut list = self.get(list_id)?;
        let entry = list
            .entries
            .iter_mut()
            .find(|e| e.id == entry_uuid)
            .ok_or_else(|| ShoppingError::NotFound(format!("entry: {entry_id}")))?;
        // No pantry write in either direction: the stock was already
        // there (or was never counted), so touching it would invent
        // quantities that don't exist.
        entry.status = if have {
            EntryStatus::Have
        } else {
            EntryStatus::Needed
        };
        self.update(list)
    }

    fn reset(&self, id: &str) -> Result<ShoppingList, ShoppingError> {
        let mut list = self.get(id)?;
        for entry in list.entries.iter_mut() {
            entry.status = EntryStatus::Needed;
        }
        self.update(list)
    }

    fn start_from_template(
        &self,
        template_id: &str,
        name: &str,
    ) -> Result<ShoppingList, ShoppingError> {
        let template = self.get(template_id)?;
        let entries = template
            .entries
            .iter()
            // Fresh entry ids: the run is its own thing, and reusing
            // the template's would make `mark_purchased` on a run
            // ambiguous across concurrent runs of the same template.
            .map(|e| ShoppingEntry {
                id: Uuid::new_v4(),
                status: EntryStatus::Needed,
                ..e.clone()
            })
            .collect::<Vec<_>>();
        self.create(ShoppingList {
            path: String::new(),
            id: Uuid::new_v4(),
            name: name.to_string(),
            store_location_id: template.store_location_id,
            entries: ShoppingEntries(entries),
            is_template: false,
            from_template: Some(template.id),
            date_created: None,
            date_modified: None,
            details: String::new(),
        })
    }

    fn save_as_template(&self, list_id: &str, name: &str) -> Result<ShoppingList, ShoppingError> {
        let source = self.get(list_id)?;
        let entries = source
            .entries
            .iter()
            .map(|e| ShoppingEntry {
                id: Uuid::new_v4(),
                status: EntryStatus::Needed,
                ..e.clone()
            })
            .collect::<Vec<_>>();
        self.create(ShoppingList {
            path: String::new(),
            id: Uuid::new_v4(),
            name: name.to_string(),
            store_location_id: source.store_location_id,
            entries: ShoppingEntries(entries),
            is_template: true,
            from_template: None,
            date_created: None,
            date_modified: None,
            details: String::new(),
        })
    }
}

fn shortage_to_entry(short: Shortage) -> ShoppingEntry {
    ShoppingEntry {
        id: Uuid::new_v4(),
        item_id: None,
        name: short.name,
        qty: Some((short.need - short.have).max(0.0)),
        unit: short.unit,
        note: Some(match short.reason {
            ShortageReason::NotInPantry => "missing — not in pantry".into(),
            ShortageReason::InsufficientQty => "low — top up".into(),
            ShortageReason::UnitMismatch => "unit mismatch — check recipe".into(),
            ShortageReason::OptionalNoQty => "optional — qty TBD".into(),
        }),
        status: EntryStatus::Needed,
    }
}
