//! File-backed [`PantryService`] impl.
//!
//! CRUD is [`vault_entity::VaultEntityStore`]; what stays here is
//! `rename` (the shared store deliberately never moves a file) and
//! the food-specific mutators — consume / restock / open, the stock
//! ledger, and barcode resolution.

use std::sync::{Arc, Mutex};

use uuid::Uuid;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::PantryItems;
use crate::model::{PantryItem, StockEntry};
use crate::parse::{looks_like_pantry_item, parse_page};
use crate::service::{BarcodeResolution, ConsumeReceipt, EntryDebit, PantryError, PantryService};

vault_entity::entity_error_bridge!(PantryError);

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    inner: VaultEntityStore<PantryItems>,
}

impl Store {
    #[must_use]
    pub fn new(vault: Vault) -> Self {
        Self {
            inner: VaultEntityStore::new(vault),
        }
    }

    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        Self {
            inner: VaultEntityStore::from_shared(inner),
        }
    }

    #[must_use]
    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.shared()
    }
}

fn map_io(e: impl std::fmt::Display) -> PantryError {
    PantryError::Io(e.to_string())
}

impl PantryService for Store {
    fn list(&self) -> Result<Vec<PantryItem>, PantryError> {
        Ok(self.inner.list())
    }

    fn get(&self, id: &str) -> Result<PantryItem, PantryError> {
        self.inner.get(id).map_err(from_entity_error)
    }

    fn create(&self, mut item: PantryItem) -> Result<PantryItem, PantryError> {
        // The `pantry` tag is the discriminator. `to_markdown` asserts
        // it on the bytes; assert it on the value too so the record the
        // caller gets back matches what landed on disk.
        if !item.tags.iter().any(|t| t == "pantry") {
            item.tags.push("pantry".into());
        }
        self.inner.create(item).map_err(from_entity_error)
    }

    fn update(&self, item: PantryItem) -> Result<PantryItem, PantryError> {
        self.inner.update(item).map_err(from_entity_error)
    }

    /// Move a page to `new_path`, keeping its bytes verbatim.
    ///
    /// Not plain CRUD — the shared store deliberately never moves a
    /// file on update — so it stays hand-written here.
    fn rename(&self, id: &str, new_path: &str) -> Result<PantryItem, PantryError> {
        let uuid = Uuid::parse_str(id).map_err(|e| PantryError::BadRequest(format!("id: {e}")))?;
        self.inner
            .with_vault_mut(|guard| -> Result<PantryItem, PantryError> {
                let idx = guard
                    .pages
                    .iter()
                    .position(|p| {
                        looks_like_pantry_item(p)
                            && parse_page(p).map(|i| i.id == uuid).unwrap_or(false)
                    })
                    .ok_or_else(|| PantryError::NotFound(id.to_string()))?;
                if guard.pages.iter().any(|p| p.rel_path == new_path) {
                    return Err(PantryError::AlreadyExists(new_path.to_string()));
                }
                let old_path = guard.pages[idx].rel_path.clone();
                let raw = guard.pages[idx].raw.clone();
                vault::delete_page(&mut *guard, &old_path).map_err(map_io)?;
                vault::create_page(&mut *guard, new_path, raw).map_err(map_io)?;
                let new_page = guard
                    .pages
                    .iter()
                    .find(|p| p.rel_path == new_path)
                    .ok_or_else(|| PantryError::Io("rename: page missing post-write".into()))?;
                parse_page(new_page).map_err(|e| PantryError::Io(e.to_string()))
            })
    }

    fn delete(&self, id: &str) -> Result<(), PantryError> {
        self.inner.delete(id).map_err(from_entity_error)
    }

    fn consume(&self, id: &str, amount: f64) -> Result<PantryItem, PantryError> {
        if amount < 0.0 {
            return Err(PantryError::BadRequest(
                "consume amount must be non-negative".into(),
            ));
        }
        let mut item = self.get(id)?;
        let have = item.qty.unwrap_or(0.0);
        if amount > have {
            return Err(PantryError::InsufficientStock {
                have,
                need: amount,
                unit: item.unit.clone(),
            });
        }
        item.qty = Some(have - amount);
        // Auto-mark as opened on first consume — most pantry
        // items are packaged, so the first scoop is also the
        // moment the shelf-life clock starts ticking.
        if !item.opened {
            item.opened = true;
            item.opened_date
                .get_or_insert_with(|| chrono::Utc::now().date_naive());
        }
        self.update(item)
    }

    fn restock(&self, id: &str, amount: f64) -> Result<PantryItem, PantryError> {
        if amount < 0.0 {
            return Err(PantryError::BadRequest(
                "restock amount must be non-negative".into(),
            ));
        }
        let mut item = self.get(id)?;
        item.qty = Some(item.qty.unwrap_or(0.0) + amount);
        self.update(item)
    }

    fn find_by_barcode(&self, barcode: &str) -> Result<PantryItem, PantryError> {
        let needle = barcode.trim();
        if needle.is_empty() {
            return Err(PantryError::BadRequest("empty barcode".into()));
        }
        self.inner
            .find(|i| i.barcodes.iter().any(|b| b == needle))
            .ok_or_else(|| PantryError::NotFound(format!("barcode: {needle}")))
    }

    fn add_stock(&self, id: &str, mut entry: StockEntry) -> Result<PantryItem, PantryError> {
        if entry.qty < 0.0 {
            return Err(PantryError::BadRequest(
                "add_stock qty must be non-negative".into(),
            ));
        }
        if entry.id.is_nil() {
            entry.id = Uuid::new_v4();
        }
        let mut item = self.get(id)?;
        // Phase-4 shelf-life: compute best_before from
        // purchased_date + default_best_before_days when
        // the caller didn't pass one.
        if entry.best_before.is_none() {
            if let Some(days) = item.default_best_before_days {
                entry.best_before = entry
                    .purchased_date
                    .checked_add_days(chrono::Days::new(u64::from(days)));
            }
        }
        item.stock_entries.push(entry);
        self.update(item)
    }

    fn consume_stock(&self, id: &str, amount: f64) -> Result<ConsumeReceipt, PantryError> {
        if amount < 0.0 {
            return Err(PantryError::BadRequest(
                "consume amount must be non-negative".into(),
            ));
        }
        let mut item = self.get(id)?;

        // Legacy fallback: when no stock entries are
        // present, deduct from the page-level qty field so
        // pre-phase-2 pantry pages keep working.
        if item.stock_entries.is_empty() {
            let have = item.qty.unwrap_or(0.0);
            if amount > have {
                return Err(PantryError::InsufficientStock {
                    have,
                    need: amount,
                    unit: item.unit.clone(),
                });
            }
            item.qty = Some(have - amount);
            let updated = self.update(item)?;
            return Ok(ConsumeReceipt {
                item: updated,
                debits: Vec::new(),
            });
        }

        let have = item.stock_total().unwrap_or(0.0);
        if amount > have {
            return Err(PantryError::InsufficientStock {
                have,
                need: amount,
                unit: item.unit.clone(),
            });
        }

        // Sort indexes FIFO: nearest best_before first, then
        // opened entries break ties (use up the opened
        // ones), then oldest purchase. `None` best_before
        // sorts last.
        let mut order: Vec<usize> = (0..item.stock_entries.len()).collect();
        order.sort_by(|&a, &b| {
            let ea = &item.stock_entries[a];
            let eb = &item.stock_entries[b];
            ea.best_before
                .map_or((true, chrono::NaiveDate::MAX), |d| (false, d))
                .cmp(
                    &eb.best_before
                        .map_or((true, chrono::NaiveDate::MAX), |d| (false, d)),
                )
                .then_with(|| eb.opened.cmp(&ea.opened))
                .then_with(|| ea.purchased_date.cmp(&eb.purchased_date))
                .then_with(|| ea.id.cmp(&eb.id))
        });

        let mut remaining = amount;
        let mut debits: Vec<EntryDebit> = Vec::new();
        let today = chrono::Utc::now().date_naive();
        let after_open = item.default_best_before_days_after_open;
        for idx in order {
            if remaining <= 0.0 {
                break;
            }
            let entry = &mut item.stock_entries[idx];
            let take = entry.qty.min(remaining);
            if take > 0.0 {
                // Phase-4 auto-open: first consume from an
                // unopened entry flips it open and tightens
                // best_before to opened_date + after_open
                // days when the new date is sooner.
                if !entry.opened {
                    entry.opened = true;
                    entry.opened_date.get_or_insert(today);
                    if let Some(days) = after_open {
                        if let Some(new_bb) = entry
                            .opened_date
                            .and_then(|d| d.checked_add_days(chrono::Days::new(u64::from(days))))
                        {
                            entry.best_before = Some(match entry.best_before {
                                Some(existing) if existing < new_bb => existing,
                                _ => new_bb,
                            });
                        }
                    }
                }
                entry.qty -= take;
                remaining -= take;
                debits.push(EntryDebit {
                    entry_id: entry.id,
                    qty: take,
                });
            }
        }

        // Keep drained entries in place for audit; callers
        // can prune via `update` if desired.
        let updated = self.update(item)?;
        Ok(ConsumeReceipt {
            item: updated,
            debits,
        })
    }

    fn transfer_stock(
        &self,
        id: &str,
        entry_id: &str,
        location_id: &str,
    ) -> Result<PantryItem, PantryError> {
        let entry_uuid = Uuid::parse_str(entry_id)
            .map_err(|e| PantryError::BadRequest(format!("entry_id: {e}")))?;
        let new_loc = if location_id.is_empty() {
            None
        } else {
            Some(
                Uuid::parse_str(location_id)
                    .map_err(|e| PantryError::BadRequest(format!("location_id: {e}")))?,
            )
        };
        let mut item = self.get(id)?;
        let entry = item
            .stock_entries
            .iter_mut()
            .find(|e| e.id == entry_uuid)
            .ok_or_else(|| PantryError::NotFound(format!("entry: {entry_id}")))?;
        entry.location_id = new_loc;
        self.update(item)
    }

    fn inventory_set(&self, id: &str, qty: f64) -> Result<PantryItem, PantryError> {
        if qty < 0.0 {
            return Err(PantryError::BadRequest(
                "inventory_set qty must be non-negative".into(),
            ));
        }
        let mut item = self.get(id)?;
        if item.stock_entries.is_empty() {
            item.qty = Some(qty);
            return self.update(item);
        }
        let have = item.stock_total().unwrap_or(0.0);
        let diff = qty - have;
        if diff.abs() < f64::EPSILON {
            return Ok(item);
        }
        let today = chrono::Utc::now().date_naive();
        if diff > 0.0 {
            item.stock_entries.push(StockEntry {
                id: Uuid::new_v4(),
                qty: diff,
                purchased_date: today,
                best_before: None,
                opened: false,
                opened_date: None,
                price: None,
                location_id: None,
                note: Some("inventory adjustment".into()),
            });
        } else {
            // Negative diff — drain via the same FIFO logic.
            let receipt = self.consume_stock(id, -diff)?;
            return Ok(receipt.item);
        }
        self.update(item)
    }

    fn resolve_barcode(&self, barcode: &str) -> Result<BarcodeResolution, PantryError> {
        match self.find_by_barcode(barcode) {
            Ok(item) => Ok(BarcodeResolution::Local(item)),
            Err(PantryError::NotFound(_)) => match crate::lookup::lookup_external(barcode) {
                Ok(Some(draft)) => Ok(BarcodeResolution::Draft(draft)),
                Ok(None) => Ok(BarcodeResolution::NotFound),
                Err(e) => Err(PantryError::Lookup(e.to_string())),
            },
            Err(e) => Err(e),
        }
    }

    fn open(&self, id: &str) -> Result<PantryItem, PantryError> {
        let mut item = self.get(id)?;
        if item.opened {
            return Ok(item);
        }
        item.opened = true;
        item.opened_date = Some(chrono::Utc::now().date_naive());
        self.update(item)
    }
}
