//! Daily food intake log — one page per day with a flat
//! list of consumed entries.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use cookbook_proto::Nutrition;
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `Vec<String>` newtype — JSON column under SeaORM.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Tags(pub Vec<String>);

impl Tags {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for Tags {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for Tags {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for Tags {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// `Option<Nutrition>` newtype — JSON column. Wraps
/// [`cookbook_proto::Nutrition`] so the wrapper carries the
/// `is_none` skip predicate the `IntakeLog` serde attrs want.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct DailyTarget(pub Option<Nutrition>);

impl DailyTarget {
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

impl From<Option<Nutrition>> for DailyTarget {
    fn from(n: Option<Nutrition>) -> Self {
        Self(n)
    }
}

impl std::ops::Deref for DailyTarget {
    type Target = Option<Nutrition>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// `Vec<IntakeEntry>` newtype — JSON column. Entries live
/// inline in the day's log.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Entries(pub Vec<IntakeEntry>);

impl Entries {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<IntakeEntry>> for Entries {
    fn from(v: Vec<IntakeEntry>) -> Self {
        Self(v)
    }
}

impl FromIterator<IntakeEntry> for Entries {
    fn from_iter<I: IntoIterator<Item = IntakeEntry>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for Entries {
    type Target = Vec<IntakeEntry>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Entries {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "intake_logs", repo)]
pub struct IntakeLog {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Display label — defaults to `"Intake <date>"` when
    /// auto-created via `log_*` shortcuts.
    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    #[architect(filterable, sortable)]
    pub date: NaiveDate,

    #[serde(skip_serializing_if = "Entries::is_empty", default)]
    #[architect(json)]
    pub entries: Entries,

    /// Daily targets — when set, callers can show
    /// progress bars without storing the goal twice. All
    /// fields optional so partial targets ("I track
    /// protein, not calories") work.
    #[serde(skip_serializing_if = "DailyTarget::is_none", default)]
    #[architect(json)]
    pub target: DailyTarget,

    /// Free-form tags — `"cut"`, `"bulk"`, `"travel-day"`,
    /// `"reset"`.
    #[serde(skip_serializing_if = "Tags::is_empty", default)]
    #[architect(json)]
    pub tags: Tags,

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

    /// Markdown body — notes, cravings, hunger ratings.
    #[serde(skip)]
    pub details: String,
}

/// One consumed item. Held inline inside [`IntakeLog::entries`]
/// as JSON; promotes to its own entity once entry-level
/// queries (e.g. "all banana intakes this month") become
/// hot enough to warrant a separate table.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct IntakeEntry {
    pub id: Uuid,

    /// What this entry references. Drives the
    /// `source_id` lookup; `Freeform` rows skip lookup
    /// and carry their nutrition inline.
    pub source: IntakeSource,

    /// Cached display name — round-trips when the
    /// referenced recipe/pantry page isn't loaded.
    pub name: String,

    /// Qty consumed *in `unit`*. For recipes, qty is
    /// "servings" and unit is conventionally `"serving"`.
    pub qty: f64,

    #[serde(default)]
    pub unit: String,

    /// Time of day, when known. `None` for catch-all
    /// "lunch" entries the user didn't bother timestamping.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub time: Option<NaiveTime>,

    /// Optional meal-slot label — `"breakfast"`,
    /// `"lunch"`, `"dinner"`, `"snack"`. Free-form so
    /// custom slots (`"pre-workout"`) round-trip.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub slot: Option<String>,

    /// Resolved nutrition for this entry. Auto-populated
    /// when the entry is added via `log_recipe` /
    /// `log_pantry` from the source's nutrition fields
    /// (scaled by `qty`). Hand-entered for `Freeform`
    /// rows. Stored on the entry so a renamed/deleted
    /// source doesn't lose the calorie data.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nutrition: Option<Nutrition>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

/// What an [`IntakeEntry`] points at.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[repr(u8)]
pub enum IntakeSource {
    /// A `cookbook::Recipe` referenced by vault-relative
    /// `.cook` path. `qty` is "servings of this recipe".
    Recipe { path: String },
    /// A `pantry::PantryItem` id. `qty` is in the
    /// pantry item's `unit` (typically the canonical
    /// stock unit).
    Pantry { id: Uuid },
    /// Hand-entered with no catalog link — restaurant
    /// food, friend's cooking, label-on-the-bag estimates.
    Freeform,
}

impl IntakeLog {
    /// Sum nutrition across every entry. Returns `None`
    /// only when no entry has nutrition data; partial
    /// coverage aggregates what's known.
    pub fn total(&self) -> Option<Nutrition> {
        let mut acc = Nutrition::default();
        let mut any = false;
        for entry in self.entries.iter() {
            let Some(n) = &entry.nutrition else {
                continue;
            };
            any = true;
            acc.calories = sum(acc.calories, n.calories);
            acc.protein_g = sum(acc.protein_g, n.protein_g);
            acc.carbs_g = sum(acc.carbs_g, n.carbs_g);
            acc.fat_g = sum(acc.fat_g, n.fat_g);
            acc.fiber_g = sum(acc.fiber_g, n.fiber_g);
            acc.sugar_g = sum(acc.sugar_g, n.sugar_g);
        }
        any.then_some(acc)
    }

    /// Convenience: every entry inside `slot` (e.g.
    /// `"breakfast"`). Slot match is case-insensitive.
    pub fn entries_in_slot(&self, slot: &str) -> Vec<&IntakeEntry> {
        let needle = slot.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                e.slot
                    .as_ref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&needle))
            })
            .collect()
    }
}

fn sum(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Scale a `Nutrition` by `factor` — used when resolving
/// recipe nutrition (per-serving × servings consumed) or
/// pantry nutrition (per-unit × qty / nutrition_unit-qty).
pub fn scale_nutrition(n: &Nutrition, factor: f64) -> Nutrition {
    Nutrition {
        calories: n.calories.map(|v| v * factor),
        protein_g: n.protein_g.map(|v| v * factor),
        carbs_g: n.carbs_g.map(|v| v * factor),
        fat_g: n.fat_g.map(|v| v * factor),
        fiber_g: n.fiber_g.map(|v| v * factor),
        sugar_g: n.sugar_g.map(|v| v * factor),
    }
}
