//! `Item` — typed view of one inventory page.
//!
//! Items live as markdown files in a `vault::Vault` under
//! `inventory/<slug>.md` (discovered via the `type: item`
//! frontmatter discriminator). Each item carries a stable
//! `id` (uuid) and references its `location_id` by id, not
//! by name, so locations can be renamed without breaking
//! the link.
//!
//! `category`, `condition`, and `status` are stored as
//! free-form strings — the [`Condition`] and [`Status`]
//! enums name the canonical sets without forcing them, so
//! custom values round-trip cleanly.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Shared `Vec<String>` newtype — JSON column under SeaORM.
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

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "items", repo)]
pub struct Item {
    /// Vault-relative path of the markdown file backing this
    /// item. Populated by the scanner; not serialized into
    /// frontmatter; stored as a DB column under server.
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    /// Stable identifier — survives renames + moves.
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Free-form category — `"guitar"`, `"amp"`, `"mic"`,
    /// `"cable"`, `"outboard"`, `"computer"`. Convention only;
    /// nothing enforces a fixed set.
    #[serde(default)]
    #[architect(filterable)]
    pub category: String,

    /// Where the item physically lives. Points at a `Location`
    /// id (see the `locations` crate). `None` for un-located
    /// items (e.g. "still in shipping").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(filterable)]
    pub location_id: Option<Uuid>,

    /// Free-form condition; canonical set in [`Condition`].
    #[serde(default = "default_condition")]
    #[architect(filterable)]
    pub condition: String,

    /// Free-form lifecycle status; canonical set in [`Status`].
    #[serde(default = "default_status")]
    #[architect(filterable)]
    pub status: String,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub manufacturer: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub serial: Option<String>,

    /// When the item was bought. Optional — much gear arrives
    /// without a paper trail.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "purchaseDate"
    )]
    pub purchase_date: Option<NaiveDate>,

    /// Approximate value in user-default currency. Free-form
    /// number — insurance/depreciation modeling lives in a
    /// future `finance` feature, not here.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<f64>,

    /// Links to task pages — repairs, follow-ups, maintenance.
    /// Stored as vault-relative paths (`Task/restring-strat.md`)
    /// or wikilinks (`"[[Restring Strat]]"`); the `task` crate's
    /// scanner resolves them.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub tasks: StringList,

    /// Free-form tags — `"primary"`, `"backup"`, `"vintage"`,
    /// `"rental"`. Drives queries; everything else is freeform.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
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

    /// Markdown body — notes, serials list, accessory list, etc.
    #[serde(skip)]
    pub details: String,
}

fn default_condition() -> String {
    Condition::Good.as_str().to_string()
}

fn default_status() -> String {
    Status::Stored.as_str().to_string()
}

/// Canonical condition values. Parsing accepts any string;
/// unknown values round-trip as raw strings on `Item.condition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Condition {
    New,
    Good,
    Fair,
    Poor,
    Broken,
}

impl Condition {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Good => "good",
            Self::Fair => "fair",
            Self::Poor => "poor",
            Self::Broken => "broken",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "new" | "mint" => Some(Self::New),
            "good" | "excellent" => Some(Self::Good),
            "fair" | "ok" | "okay" => Some(Self::Fair),
            "poor" | "bad" => Some(Self::Poor),
            "broken" | "dead" | "non-functional" => Some(Self::Broken),
            _ => None,
        }
    }

    /// `true` for conditions that should surface a "needs
    /// attention" affordance in UIs.
    #[must_use]
    pub fn needs_attention(self) -> bool {
        matches!(self, Self::Poor | Self::Broken)
    }
}

/// Canonical lifecycle status. Free-form on the wire so
/// custom workflows survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    InUse,
    Stored,
    Loaned,
    InRepair,
    Missing,
    Retired,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InUse => "in-use",
            Self::Stored => "stored",
            Self::Loaned => "loaned",
            Self::InRepair => "in-repair",
            Self::Missing => "missing",
            Self::Retired => "retired",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "in-use" | "in_use" | "using" | "active" => Some(Self::InUse),
            "stored" | "storage" | "shelf" => Some(Self::Stored),
            "loaned" | "lent" | "out" => Some(Self::Loaned),
            "in-repair" | "in_repair" | "repair" | "fixing" => Some(Self::InRepair),
            "missing" | "lost" => Some(Self::Missing),
            "retired" | "sold" | "donated" => Some(Self::Retired),
            _ => None,
        }
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Item {
    type Key = uuid::Uuid;
    fn key(&self) -> uuid::Uuid {
        self.id
    }
}
