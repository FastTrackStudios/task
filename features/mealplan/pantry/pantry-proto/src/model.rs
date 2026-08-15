//! `PantryItem` — typed view of one food-on-hand page.
//!
//! A pantry page IS an inventory page: same `type: item`
//! discriminator + same `id` so `inventory::looks_like_item`
//! returns true, and `pantry` in `tags:` so this crate can
//! pick the food rows out of the broader inventory list. One
//! physical thing = one markdown file visible in both lists;
//! no shadow rows, no parallel uuid.
//!
//! This wire model lives in the wasm-clean `pantry-proto` crate
//! so the web UI can bind the `PantryService` client directly.
//! The `to_item` / `from_item` interop with the (vault-backed,
//! non-wasm) `inventory::Item` model is gated behind the `native`
//! feature so the wasm build never pulls `inventory`.

use chrono::{DateTime, NaiveDate, Utc};
use cookbook_proto::Nutrition;
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `Vec<String>` newtype — JSON column. Used for `tags` and
/// `barcodes`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct StringList(pub Vec<String>);

impl StringList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for StringList {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for StringList {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for StringList {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for StringList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// `Vec<StockEntry>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct StockEntries(pub Vec<StockEntry>);

impl StockEntries {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<StockEntry>> for StockEntries {
    fn from(v: Vec<StockEntry>) -> Self {
        Self(v)
    }
}

impl FromIterator<StockEntry> for StockEntries {
    fn from_iter<I: IntoIterator<Item = StockEntry>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for StockEntries {
    type Target = Vec<StockEntry>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for StockEntries {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// `Vec<Substitution>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Substitutions(pub Vec<Substitution>);

impl Substitutions {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<Substitution>> for Substitutions {
    fn from(v: Vec<Substitution>) -> Self {
        Self(v)
    }
}

impl FromIterator<Substitution> for Substitutions {
    fn from_iter<I: IntoIterator<Item = Substitution>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for Substitutions {
    type Target = Vec<Substitution>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "pantry_items", repo)]
pub struct PantryItem {
    // ── Identity + locator (mirrors inventory::Item so a
    // pantry page round-trips through inventory's scanner) ──
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Free-form. Pantry pages set this to `"food"` by
    /// convention — finer-grained food taxonomy lives in
    /// [`Self::food_category`].
    #[serde(default = "default_category")]
    #[architect(filterable)]
    pub category: String,

    /// Where the item physically lives — `locations::Location`
    /// id. Pantry items typically point at a "Kitchen Pantry"
    /// or "Fridge" sub-location.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(filterable)]
    pub location_id: Option<Uuid>,

    #[serde(default = "default_condition")]
    #[architect(filterable)]
    pub condition: String,

    #[serde(default = "default_status")]
    #[architect(filterable)]
    pub status: String,

    /// Inventory tags. Always contains `"pantry"` for pages
    /// owned by this crate; round-trip preserves the rest.
    #[serde(default)]
    #[architect(json)]
    pub tags: StringList,

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

    // ── Food-specific fields (the reason this crate exists) ──
    /// Free-form food category. Canonical set in
    /// [`FoodCategory`].
    #[serde(default, rename = "foodCategory")]
    pub food_category: String,

    /// Current amount on hand. `None` for "I have some but
    /// haven't measured" — surface in the UI, don't guess.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub qty: Option<f64>,

    /// Canonical **stock unit** — how the item is measured
    /// once it's on the shelf. Free-form so unknown units
    /// round-trip; the common ones (`g` / `ml` / `cup` / `each`
    /// / `clove` / `bunch` / etc.) are resolved by the native
    /// `pantry::units` layer. Recipe deductions match against
    /// this unit (with conversion when bases align).
    #[serde(default)]
    pub unit: String,

    /// Optional **purchase unit** — what you buy in,
    /// when it differs from the stock unit. Buy by the
    /// `"box"`, store by the `"can"`; record both so the
    /// shopping list reads in the purchase unit but the
    /// recipe math runs in stock units. Pair with
    /// [`Self::purchase_to_stock_factor`].
    ///
    /// Modeled on grocy's `qu_id_purchase` /
    /// `qu_id_stock` / `qu_factor_purchase_to_stock`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "purchaseUnit"
    )]
    pub purchase_unit: Option<String>,

    /// Multiplier: `1 purchase_unit ==
    /// purchase_to_stock_factor stock_units`. Required
    /// when `purchase_unit` is set; ignored otherwise. For
    /// "box of 24 cans": `purchase_unit: "box"`,
    /// `unit: "can"`, `purchase_to_stock_factor: 24.0`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "purchaseToStockFactor"
    )]
    pub purchase_to_stock_factor: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expiry: Option<NaiveDate>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "openedDate"
    )]
    pub opened_date: Option<NaiveDate>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub brand: Option<String>,

    /// Nutrition per 1 [`Self::nutrition_unit`] (defaults to
    /// `unit` when unset). Recipe scaling + (future) fitness
    /// calorie logs read this. Stored as JSON via
    /// `cookbook::Nutrition`'s `JsonField` derive.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "nutritionPerUnit"
    )]
    #[architect(json)]
    pub nutrition_per_unit: Option<Nutrition>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "nutritionUnit"
    )]
    pub nutrition_unit: Option<String>,

    /// Reorder threshold. When `qty <= minimum`, surface a
    /// restock task. Same units as `qty`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub minimum: Option<f64>,

    /// Default shelf life *from purchase*. When set,
    /// [`crate::PantryService::add_stock`] auto-fills
    /// `best_before = purchased_date + days` for entries
    /// added without an explicit date. Modeled on grocy's
    /// `default_best_before_days`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "defaultBestBeforeDays"
    )]
    pub default_best_before_days: Option<u32>,

    /// Default shelf life *from open*. When set,
    /// opening an entry (via [`crate::PantryService::open`]
    /// or auto-open on first consume) tightens its
    /// `best_before` to `opened_date + days` — but only
    /// when the new date is *sooner* than the existing
    /// one. Mirrors grocy's
    /// `default_best_before_days_after_open`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "defaultBestBeforeDaysAfterOpen"
    )]
    pub default_best_before_days_after_open: Option<u32>,

    /// Default shelf life *from freezing*. Future-use field
    /// — reserved for a freeze/thaw flow.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "defaultBestBeforeDaysAfterFreezing"
    )]
    pub default_best_before_days_after_freezing: Option<u32>,

    /// Default shelf life *from thaw*. Future-use field.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "defaultBestBeforeDaysAfterThawing"
    )]
    pub default_best_before_days_after_thawing: Option<u32>,

    /// `"best-before"` (soft — tastes fine past the date)
    /// vs `"expires"` (hard — toss past the date). Free-form
    /// so custom values (`"use-by"`, `"sell-by"`) round-trip;
    /// canonical set in [`DueType`].
    #[serde(default = "default_due_type", rename = "dueType")]
    pub due_type: String,

    /// Per-purchase batch entries. Each entry has its own
    /// `qty` + `best_before` + `opened` state — modeled on
    /// grocy's `stock` table. The page-level [`Self::qty`]
    /// remains a fallback for un-batched legacy pages but
    /// is otherwise treated as a derived sum (see
    /// [`Self::stock_total`]).
    #[serde(
        skip_serializing_if = "StockEntries::is_empty",
        default,
        rename = "stockEntries"
    )]
    #[architect(json)]
    pub stock_entries: StockEntries,

    /// Global substitution candidates *for this item*. Edit
    /// "Butter" once and every recipe that calls for butter
    /// can see the alternative. Fulfillment consults these
    /// after recipe-level subs and before the
    /// `mealplan::substitutions` registry. See
    /// [`Substitution`] for the structure;
    /// [`SubReason`] for the goal taxonomy.
    #[serde(skip_serializing_if = "Substitutions::is_empty", default)]
    #[architect(json)]
    pub substitutes: Substitutions,

    /// All barcodes that resolve to this pantry item. One
    /// physical food often carries N barcodes (single-pack
    /// vs multi-pack, regional variants, etc.) — grocy's
    /// `product_barcodes` table is the precedent. Scanning
    /// any of these via the native `pantry::lookup` or
    /// [`crate::PantryService::find_by_barcode`] resolves
    /// to the same row.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub barcodes: StringList,

    /// Optional product picture — typically populated from
    /// the `OpenFoodFacts` lookup. URL or vault-relative
    /// path; consumers display verbatim.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "imageUrl")]
    pub image_url: Option<String>,

    #[serde(skip)]
    pub details: String,
}

/// Partly-populated pantry row returned from an external
/// barcode lookup (see the native `pantry::lookup`). Callers
/// decide whether to merge into an existing [`PantryItem`] or
/// create a new one — the draft never touches the vault.
///
/// Only the food-level fields are populated; identity
/// (id / location / qty / expiry / etc.) is the caller's
/// responsibility because those reflect *your* physical
/// stock, not the lookup source's catalog.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct PantryItemDraft {
    pub barcode: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub brand: Option<String>,
    #[serde(default, rename = "foodCategory")]
    pub food_category: String,
    #[serde(default)]
    pub unit: String,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "nutritionPerUnit"
    )]
    pub nutrition_per_unit: Option<Nutrition>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "nutritionUnit"
    )]
    pub nutrition_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "imageUrl")]
    pub image_url: Option<String>,
}

impl PantryItemDraft {
    /// Lift the draft into a brand-new [`PantryItem`]
    /// ready for [`crate::PantryService::create`]. The
    /// barcode is added to `barcodes`. Identity (id) is
    /// left nil so the store can assign one.
    #[must_use]
    pub fn into_item(self, location_id: Option<uuid::Uuid>) -> PantryItem {
        PantryItem {
            path: String::new(),
            id: uuid::Uuid::nil(),
            name: self.name,
            category: default_category(),
            location_id,
            condition: default_condition(),
            status: default_status(),
            tags: StringList(vec!["item".into(), "pantry".into()]),
            date_created: None,
            date_modified: None,
            food_category: self.food_category,
            qty: None,
            unit: self.unit,
            purchase_unit: None,
            purchase_to_stock_factor: None,
            expiry: None,
            opened: false,
            opened_date: None,
            brand: self.brand,
            nutrition_per_unit: self.nutrition_per_unit,
            nutrition_unit: self.nutrition_unit,
            minimum: None,
            default_best_before_days: None,
            default_best_before_days_after_open: None,
            default_best_before_days_after_freezing: None,
            default_best_before_days_after_thawing: None,
            due_type: default_due_type(),
            substitutes: Substitutions::default(),
            stock_entries: StockEntries::default(),
            barcodes: StringList(vec![self.barcode]),
            image_url: self.image_url,
            details: String::new(),
        }
    }
}

fn default_category() -> String {
    "food".into()
}

fn default_condition() -> String {
    "good".into()
}

fn default_status() -> String {
    "stored".into()
}

fn default_due_type() -> String {
    DueType::BestBefore.as_str().to_string()
}

/// Canonical [`PantryItem::due_type`] values. Free-form on
/// the wire; this enum names the recognized ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DueType {
    /// Soft date — quality drops past it, but the food is
    /// still safe. Drives the "expiring soon" surface, not
    /// the "throw it out" surface.
    BestBefore,
    /// Hard date — toss past it. Surfaces a stronger
    /// warning + auto-suggests adding to the shopping list.
    Expires,
}

impl DueType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BestBefore => "best-before",
            Self::Expires => "expires",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "best-before" | "best_before" | "bestbefore" | "soft" => Some(Self::BestBefore),
            "expires" | "expire" | "expiry" | "use-by" | "useby" | "hard" => Some(Self::Expires),
            _ => None,
        }
    }
}

/// One per-purchase batch row inside a [`PantryItem`].
///
/// Modeled on grocy's `stock` table: each entry tracks the
/// remaining qty + `best_before` + `opened/opened_date` + price
/// for one physical package or one received quantity. FIFO
/// consume across entries powers shelf-life-aware
/// deductions; transferring between locations swaps the
/// `location_id` without splitting the row.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct StockEntry {
    /// Stable batch id. Survives splits / merges.
    pub id: Uuid,

    /// Remaining amount. Same `unit` as the parent
    /// [`PantryItem::unit`].
    pub qty: f64,

    #[serde(rename = "purchasedDate")]
    pub purchased_date: NaiveDate,

    /// Best-before / use-by date. `None` for items that
    /// don't carry one (salt, sugar, etc.).
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "bestBefore"
    )]
    pub best_before: Option<NaiveDate>,

    /// `true` once the package has been opened — shelf
    /// life often shifts at this point (see
    /// [`PantryItem::default_best_before_days_after_open`]
    /// once phase 4 lands).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "openedDate"
    )]
    pub opened_date: Option<NaiveDate>,

    /// Purchase price in user-default currency. Used for
    /// price-history queries; never required.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<f64>,

    /// Where this batch lives. Defaults to the parent
    /// `PantryItem::location_id` when unset — only override
    /// when the item is split across two locations
    /// (pantry + freezer, etc.).
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "locationId"
    )]
    pub location_id: Option<Uuid>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

impl StockEntry {
    #[must_use]
    pub fn is_expired(&self, today: NaiveDate) -> bool {
        self.best_before.is_some_and(|d| d < today)
    }
}

/// Pantry-level substitution candidate. Lives on
/// [`PantryItem::substitutes`] — "I can use this instead of
/// that".
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct Substitution {
    /// The substitute pantry item.
    #[serde(rename = "itemId")]
    pub item_id: Uuid,

    /// `units_of_substitute / unit_of_original`. `1.0` =
    /// 1:1; `0.75` = "use 75% as much"; `2.0` = double.
    #[serde(default = "default_ratio")]
    pub ratio: f64,

    /// Why this swap is worth suggesting. Multiple reasons
    /// are common — coconut oil for butter is both
    /// `Vegan` and `LowerCalorie` if you like.
    #[serde(default)]
    pub reasons: Vec<SubReason>,

    /// Optional caveat — `"adds sweetness"`, `"changes
    /// browning"`, `"may need extra acid"`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

fn default_ratio() -> f64 {
    1.0
}

/// Why a substitution is worth offering. Shared with
/// goal-filtering — caller passes `Vec<SubReason>` as their
/// preferences and matching subs float up.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum SubReason {
    /// The original is out of stock. Always relevant; this
    /// is the default goal when fulfillment surfaces a
    /// shortage.
    OutOfStock,
    LowerCalorie,
    LowerFat,
    LowerCarb,
    LowerSugar,
    LowerSodium,
    HigherProtein,
    HigherFiber,
    Vegan,
    Vegetarian,
    GlutenFree,
    DairyFree,
    NutFree,
    Cheaper,
    /// "I just prefer it" — catch-all so personal
    /// preferences don't have to be shoehorned into a
    /// health bucket.
    Preference,
}

impl SubReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutOfStock => "out-of-stock",
            Self::LowerCalorie => "lower-calorie",
            Self::LowerFat => "lower-fat",
            Self::LowerCarb => "lower-carb",
            Self::LowerSugar => "lower-sugar",
            Self::LowerSodium => "lower-sodium",
            Self::HigherProtein => "higher-protein",
            Self::HigherFiber => "higher-fiber",
            Self::Vegan => "vegan",
            Self::Vegetarian => "vegetarian",
            Self::GlutenFree => "gluten-free",
            Self::DairyFree => "dairy-free",
            Self::NutFree => "nut-free",
            Self::Cheaper => "cheaper",
            Self::Preference => "preference",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "out-of-stock" | "out_of_stock" | "oos" => Some(Self::OutOfStock),
            "lower-calorie" | "lower-calories" | "lower-cal" => Some(Self::LowerCalorie),
            "lower-fat" | "low-fat" => Some(Self::LowerFat),
            "lower-carb" | "low-carb" | "keto" => Some(Self::LowerCarb),
            "lower-sugar" | "low-sugar" => Some(Self::LowerSugar),
            "lower-sodium" | "low-sodium" | "low-salt" => Some(Self::LowerSodium),
            "higher-protein" | "high-protein" => Some(Self::HigherProtein),
            "higher-fiber" | "high-fiber" => Some(Self::HigherFiber),
            "vegan" => Some(Self::Vegan),
            "vegetarian" | "veggie" => Some(Self::Vegetarian),
            "gluten-free" | "gf" => Some(Self::GlutenFree),
            "dairy-free" | "df" => Some(Self::DairyFree),
            "nut-free" => Some(Self::NutFree),
            "cheaper" | "budget" => Some(Self::Cheaper),
            "preference" | "pref" => Some(Self::Preference),
            _ => None,
        }
    }
}

impl PantryItem {
    /// Total `qty` across all batch entries. Falls back to
    /// the legacy page-level [`Self::qty`] when no entries
    /// are present.
    #[must_use]
    pub fn stock_total(&self) -> Option<f64> {
        if self.stock_entries.is_empty() {
            self.qty
        } else {
            Some(self.stock_entries.iter().map(|e| e.qty).sum())
        }
    }

    #[must_use]
    pub fn is_expired(&self, today: NaiveDate) -> bool {
        self.expiry.is_some_and(|d| d < today)
    }

    #[must_use]
    pub fn is_low(&self) -> bool {
        match (self.qty, self.minimum) {
            (Some(q), Some(m)) => q <= m,
            _ => false,
        }
    }
}

/// Interop with the gear-inventory model. Native only — the
/// `inventory` crate is `cfg(not(wasm))`, so these helpers are
/// behind the `native` feature and never reach the wasm UI.
#[cfg(feature = "native")]
impl PantryItem {
    /// Lossy down-conversion to the inventory model. Drops
    /// the food-specific fields; useful when handing a row
    /// off to the gear-inventory UI.
    #[must_use]
    pub fn to_item(&self) -> inventory::Item {
        inventory::Item {
            path: self.path.clone(),
            id: self.id,
            name: self.name.clone(),
            category: self.category.clone(),
            location_id: self.location_id,
            condition: self.condition.clone(),
            status: self.status.clone(),
            manufacturer: None,
            model: None,
            serial: None,
            purchase_date: None,
            value: None,
            tasks: inventory::model::StringList::default(),
            tags: inventory::model::StringList(self.tags.0.clone()),
            date_created: self.date_created,
            date_modified: self.date_modified,
            details: self.details.clone(),
        }
    }

    /// Lift an existing inventory row into a pantry shape.
    /// Food fields start empty; the caller fills them in
    /// before persisting via the native `pantry::Store::create`.
    #[must_use]
    pub fn from_item(item: inventory::Item) -> Self {
        let mut tags = item.tags.0.clone();
        if !tags.iter().any(|t| t == "pantry") {
            tags.push("pantry".to_string());
        }
        Self {
            path: item.path,
            id: item.id,
            name: item.name,
            category: if item.category.is_empty() {
                default_category()
            } else {
                item.category
            },
            location_id: item.location_id,
            condition: item.condition,
            status: item.status,
            tags: StringList(tags),
            date_created: item.date_created,
            date_modified: item.date_modified,
            food_category: String::new(),
            qty: None,
            unit: String::new(),
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
            due_type: default_due_type(),
            substitutes: Substitutions::default(),
            stock_entries: StockEntries::default(),
            barcodes: StringList::default(),
            image_url: None,
            details: item.details,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FoodCategory {
    Produce,
    Dairy,
    Grain,
    Protein,
    Spice,
    Condiment,
    Oil,
    Canned,
    Frozen,
    Baking,
    Snack,
    Drink,
    Other,
}

impl FoodCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Produce => "produce",
            Self::Dairy => "dairy",
            Self::Grain => "grain",
            Self::Protein => "protein",
            Self::Spice => "spice",
            Self::Condiment => "condiment",
            Self::Oil => "oil",
            Self::Canned => "canned",
            Self::Frozen => "frozen",
            Self::Baking => "baking",
            Self::Snack => "snack",
            Self::Drink => "drink",
            Self::Other => "other",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "produce" | "veg" | "vegetable" | "fruit" => Some(Self::Produce),
            "dairy" => Some(Self::Dairy),
            "grain" | "carbs" | "pasta" | "rice" | "bread" => Some(Self::Grain),
            "protein" | "meat" | "fish" | "egg" | "eggs" => Some(Self::Protein),
            "spice" | "herb" | "seasoning" => Some(Self::Spice),
            "condiment" | "sauce" => Some(Self::Condiment),
            "oil" | "fat" => Some(Self::Oil),
            "canned" | "tinned" => Some(Self::Canned),
            "frozen" => Some(Self::Frozen),
            "baking" => Some(Self::Baking),
            "snack" => Some(Self::Snack),
            "drink" | "beverage" => Some(Self::Drink),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for PantryItem {
    type Key = uuid::Uuid;
    fn key(&self) -> uuid::Uuid {
        self.id
    }
}
