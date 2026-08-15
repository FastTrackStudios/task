//! Shopping-list wire contract — the model and the
//! [`ShoppingService`] trait, wasm-clean so the web UI binds the
//! client directly. The sibling `mealplan` crate owns the parse /
//! serialize / vault-backed `Store` and re-exports everything here.
//!
//! A list is walked in two passes, which is why an entry has a
//! [`EntryStatus`] rather than a `purchased` flag: first round the
//! kitchen ticking off what's already on the shelf, then to the shop
//! for whatever is left. Both passes end in "got it", but only the
//! second adds stock to the pantry.
//!
//! A list marked [`ShoppingList::is_template`] is a checklist you keep
//! rather than a run you tick off — start it fresh each time with
//! [`ShoppingService::start_from_template`].

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// `Vec<ShoppingEntry>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct ShoppingEntries(pub Vec<ShoppingEntry>);

impl ShoppingEntries {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<ShoppingEntry>> for ShoppingEntries {
    fn from(v: Vec<ShoppingEntry>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for ShoppingEntries {
    type Target = Vec<ShoppingEntry>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ShoppingEntries {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "shopping_lists", repo)]
pub struct ShoppingList {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Optional default store (a `locations::Location` of
    /// `kind: venue`). Lets the UI group lists by where
    /// you'll shop.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "storeLocationId"
    )]
    #[architect(filterable)]
    pub store_location_id: Option<Uuid>,

    #[serde(default)]
    #[architect(json)]
    pub entries: ShoppingEntries,

    /// A reusable checklist rather than a live run. Templates are never
    /// checked off in place — [`ShoppingService::start_from_template`]
    /// copies one into a fresh run, so the staples list survives every
    /// shop and can be started again next week.
    #[serde(
        default,
        skip_serializing_if = "std::ops::Not::not",
        rename = "isTemplate"
    )]
    #[architect(filterable)]
    pub is_template: bool,

    /// The template this run was started from, when it was. Lets the UI
    /// offer "start this again" and keeps provenance for a run whose
    /// template later changes.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "fromTemplate"
    )]
    #[architect(filterable)]
    pub from_template: Option<Uuid>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateCreated"
    )]
    pub date_created: Option<DateTime<Utc>>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateModified"
    )]
    pub date_modified: Option<DateTime<Utc>>,

    #[serde(skip)]
    pub details: String,
}

impl ShoppingList {
    /// Rows still to find or buy.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_settled()).count()
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct ShoppingEntry {
    pub id: Uuid,

    /// Optional `pantry::PantryItem` id. When set,
    /// `mark_purchased` will call
    /// `pantry::PantryService::add_stock` against this id
    /// so the row lands in stock immediately.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "itemId")]
    pub item_id: Option<Uuid>,

    /// Display name. Required even when `item_id` is set —
    /// the list reads independently of the pantry catalog.
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub qty: Option<f64>,

    #[serde(default)]
    pub unit: String,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,

    #[serde(default)]
    pub status: EntryStatus,
}

impl ShoppingEntry {
    /// Nothing left to do for this row — either it turned up in the
    /// kitchen or it came home from the shop.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.status != EntryStatus::Needed
    }
}

/// Where a row is in the two-stage run: check the kitchen first, then
/// buy what didn't turn up.
///
/// [`EntryStatus::Have`] and [`EntryStatus::Purchased`] both mean "got
/// it", but they are not interchangeable — only a purchase adds stock
/// to the pantry, because finding a jar you already owned doesn't
/// create any. Keeping them apart is also what lets a run be re-checked
/// later: reset to [`EntryStatus::Needed`] and walk the kitchen again.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum EntryStatus {
    /// Still to find or buy.
    #[default]
    Needed,
    /// Already in the kitchen — don't buy it.
    Have,
    /// Bought. Restocks the pantry when the row has an `item_id`.
    Purchased,
}

impl EntryStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Needed => "needed",
            Self::Have => "have",
            Self::Purchased => "purchased",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "needed" | "todo" => Some(Self::Needed),
            "have" | "in-stock" | "got" => Some(Self::Have),
            "purchased" | "bought" => Some(Self::Purchased),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum ShoppingError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("pantry: {0}")]
    Pantry(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait ShoppingService {
    fn list(&self) -> Result<Vec<ShoppingList>, ShoppingError>;

    fn get(&self, id: &str) -> Result<ShoppingList, ShoppingError>;

    fn create(&self, list: ShoppingList) -> Result<ShoppingList, ShoppingError>;

    fn update(&self, list: ShoppingList) -> Result<ShoppingList, ShoppingError>;

    fn delete(&self, id: &str) -> Result<(), ShoppingError>;

    /// Add every shortage from `recipe`'s fulfillment to
    /// `list_id`. `recipe_path` is the vault-relative
    /// `.cook` file path.
    fn add_missing_for_recipe(
        &self,
        list_id: &str,
        recipe_path: &str,
        servings: u32,
    ) -> Result<ShoppingList, ShoppingError>;

    /// Add *every* ingredient the recipe calls for at `servings`,
    /// regardless of what the pantry says.
    ///
    /// The counterpart to [`ShoppingService::add_missing_for_recipe`],
    /// and the right one for building a gather checklist: that method
    /// does the kitchen pass for you against recorded stock, which is
    /// only as good as how faithfully the pantry has been kept. This
    /// one lists the lot and lets the first pass be an actual look at
    /// an actual shelf — what doesn't turn up is the shopping list.
    fn add_recipe_ingredients(
        &self,
        list_id: &str,
        recipe_path: &str,
        servings: u32,
    ) -> Result<ShoppingList, ShoppingError>;

    /// Add every pantry item at or below its `minimum`
    /// reorder threshold to `list_id`.
    fn add_low_stock(&self, list_id: &str) -> Result<ShoppingList, ShoppingError>;

    /// Add every pantry item with a stock entry already
    /// past its `best_before` as of `today`.
    fn add_expired_or_overdue(
        &self,
        list_id: &str,
        today: NaiveDate,
    ) -> Result<ShoppingList, ShoppingError>;

    /// Drop all entries from the list (keeps the list
    /// itself). Useful after a grocery run.
    fn clear(&self, id: &str) -> Result<ShoppingList, ShoppingError>;

    /// Mark `entry_id` as purchased. When the entry has an
    /// `item_id`, also calls
    /// `pantry::PantryService::add_stock` against that id
    /// (creates a single batch row using `entry.qty` and
    /// today's date as `purchased_date`).
    fn mark_purchased(&self, list_id: &str, entry_id: &str) -> Result<ShoppingList, ShoppingError>;

    /// Mark `entry_id` as already in the kitchen — the first pass,
    /// walking the shelves before leaving. Deliberately does *not*
    /// touch the pantry: finding something you already owned doesn't
    /// add stock. Pass `have = false` to put a row back on the
    /// shopping list after a miscount.
    fn mark_have(
        &self,
        list_id: &str,
        entry_id: &str,
        have: bool,
    ) -> Result<ShoppingList, ShoppingError>;

    /// Put every row back to [`EntryStatus::Needed`], keeping the list
    /// itself. This is what makes a list re-runnable: check the same
    /// staples again next week without retyping them. Unlike
    /// [`ShoppingService::clear`], the entries survive.
    fn reset(&self, id: &str) -> Result<ShoppingList, ShoppingError>;

    /// Start a fresh run from a template: same rows, new entry ids, all
    /// back to [`EntryStatus::Needed`], `from_template` set. The
    /// template is untouched, so it can be started again next week.
    fn start_from_template(
        &self,
        template_id: &str,
        name: &str,
    ) -> Result<ShoppingList, ShoppingError>;

    /// Save `list_id`'s rows as a reusable template under `name`. The
    /// run is left alone; the copy is stored with every row reset, so
    /// starting it later begins from a clean checklist.
    fn save_as_template(&self, list_id: &str, name: &str) -> Result<ShoppingList, ShoppingError>;
}
