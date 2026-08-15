//! `PantryService` — wire surface for the pantry. Same
//! shape as `CookbookService` / `InventoryService`, plus
//! pantry-specific convenience mutators (`consume`,
//! `restock`, `open`, `set_expiry`).

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{PantryItem, PantryItemDraft, StockEntry};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum PantryError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("insufficient stock: have {have} {unit}, need {need} {unit}")]
    InsufficientStock { have: f64, need: f64, unit: String },
    #[error("lookup: {0}")]
    Lookup(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait PantryService {
    fn list(&self) -> Result<Vec<PantryItem>, PantryError>;

    fn get(&self, id: &str) -> Result<PantryItem, PantryError>;

    fn create(&self, item: PantryItem) -> Result<PantryItem, PantryError>;

    fn update(&self, item: PantryItem) -> Result<PantryItem, PantryError>;

    fn rename(&self, id: &str, new_path: &str) -> Result<PantryItem, PantryError>;

    fn delete(&self, id: &str) -> Result<(), PantryError>;

    /// Deduct `amount` from this item's `qty`. Errors with
    /// [`PantryError::InsufficientStock`] when the deduction
    /// would underflow. Used by mealplan's "I cooked this"
    /// flow.
    fn consume(&self, id: &str, amount: f64) -> Result<PantryItem, PantryError>;

    /// Add `amount` to this item's `qty`. Used by the
    /// "restock from grocery run" flow.
    fn restock(&self, id: &str, amount: f64) -> Result<PantryItem, PantryError>;

    /// Mark the package as opened and stamp today's date
    /// onto `opened_date`. Idempotent — calling on an
    /// already-opened item is a no-op.
    fn open(&self, id: &str) -> Result<PantryItem, PantryError>;

    /// Find a pantry row that already lists `barcode` in
    /// its `barcodes` field. `NotFound` if no local match
    /// — the caller can fall back to
    /// [`Self::resolve_barcode`] for an external lookup.
    fn find_by_barcode(&self, barcode: &str) -> Result<PantryItem, PantryError>;

    /// Resolve a barcode against the local vault first,
    /// then `OpenFoodFacts` as a fallback. The result tells
    /// the caller whether it's a known item (`Local`), an
    /// off-the-shelf draft ready to confirm (`Draft`), or
    /// nothing was found (`NotFound`).
    fn resolve_barcode(&self, barcode: &str) -> Result<BarcodeResolution, PantryError>;

    /// Push a new batch onto this item's stock entries.
    /// Stamps `entry.id` if nil; returns the updated item.
    /// Use this in preference to bumping `qty` directly
    /// once stock entries exist for the item — the FIFO
    /// consume math depends on entries.
    fn add_stock(&self, id: &str, entry: StockEntry) -> Result<PantryItem, PantryError>;

    /// Deduct `amount` of stock FIFO across entries. Order:
    /// nearest `best_before` first, then oldest purchase, with
    /// opened entries breaking ties (use them up first).
    /// When no stock entries are present, falls back to the
    /// page-level `qty` field (legacy path).
    ///
    /// Returns the updated item plus the per-entry
    /// debit summary so callers (mealplan cook, manual
    /// adjustments) can record what came from where.
    fn consume_stock(&self, id: &str, amount: f64) -> Result<ConsumeReceipt, PantryError>;

    /// Move one stock entry to a different location. The
    /// entry's qty / `best_before` / opened state are
    /// preserved. Pass empty `location_id` to clear it.
    fn transfer_stock(
        &self,
        id: &str,
        entry_id: &str,
        location_id: &str,
    ) -> Result<PantryItem, PantryError>;

    /// Correct the absolute total to `qty`. Adds a single
    /// corrective entry (positive or negative as needed) so
    /// the audit trail remembers the manual count. Returns
    /// the updated item.
    fn inventory_set(&self, id: &str, qty: f64) -> Result<PantryItem, PantryError>;
}

/// Per-call result of [`PantryService::consume_stock`] — the
/// updated item plus the per-entry debit detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct ConsumeReceipt {
    pub item: PantryItem,
    /// Each row: (`entry_id`, `qty_debited`). Empty entries
    /// (qty fully drained) are kept in the item with
    /// `qty: 0.0` for audit; callers can prune later.
    pub debits: Vec<EntryDebit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct EntryDebit {
    #[serde(rename = "entryId")]
    pub entry_id: uuid::Uuid,
    pub qty: f64,
}

/// Outcome of [`PantryService::resolve_barcode`].
//
// `PantryItem` + `PantryItemDraft` are both large enums; the
// size difference vs. `NotFound` is intentional — wrapping
// either in `Box<>` to silence clippy would obscure the
// happy path here, which is read-heavy and almost always
// returns `Local(_)`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum BarcodeResolution {
    /// Barcode matches a pantry item already in the vault.
    Local(PantryItem),
    /// Not in the vault, but an external source returned a
    /// draft the caller can edit + persist via
    /// [`PantryService::create`] (use
    /// [`PantryItemDraft::into_item`]).
    Draft(PantryItemDraft),
    /// Neither local vault nor external lookup found the
    /// barcode.
    NotFound,
}
