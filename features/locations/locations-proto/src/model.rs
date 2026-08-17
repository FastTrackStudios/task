//! `Location` — typed view of one place page.
//!
//! Locations live as markdown files in a `vault::Vault` under
//! `locations/<slug>.md` (or anywhere — discovered via the
//! `type: location` frontmatter discriminator). Other features
//! (notably `inventory`) reference locations by `id` so renames
//! don't break links.
//!
//! `kind` is free-form (`studio`, `room`, `storage`, `venue`,
//! `home`, ...) so user conventions round-trip; the [`Kind`]
//! enum names the canonical set without forcing it.
//!
//! Unknown frontmatter keys round-trip on the source
//! `VaultPage`, not on `Location` — keep the typed surface
//! narrow and reach for raw frontmatter when the user needs it.

use chrono::{DateTime, Utc};
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

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "locations", repo)]
pub struct Location {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Free-form. Canonical set in [`Kind`].
    #[serde(default = "default_kind")]
    #[architect(filterable)]
    pub kind: String,

    /// Parent location, if this is nested inside another (e.g.
    /// "Control Room" inside "Home Studio"). `None` for top-level.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(filterable)]
    pub parent_id: Option<Uuid>,

    /// Optional postal address (used by venue/studio kinds).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub address: Option<String>,

    /// Free-form tags — `"primary"`, `"rental"`, `"climate-controlled"`.
    #[serde(skip_serializing_if = "Tags::is_empty", default)]
    #[architect(json)]
    pub tags: Tags,

    /// Federation pointer — `Some("@codywright/home-studio")`
    /// means this row is a *reference* to the canonical
    /// location owned by another org, not an independent
    /// place. Resolver follows the link for full details
    /// (address, parent, photos). `None` for locally-owned
    /// rows.
    ///
    /// By convention the same value is also dropped into
    /// [`details`] as a `[[@org/slug]]` wikilink so the page
    /// stays readable in vanilla Obsidian.
    ///
    /// See the federated-platform design § federated
    /// wiki resolution for the `@org/slug` syntax.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "sameAs")]
    #[architect(filterable)]
    pub same_as: Option<String>,

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

    /// Markdown body after the frontmatter close fence.
    /// Free-form notes — directions, gear quirks, access codes.
    #[serde(skip)]
    pub details: String,
}

fn default_kind() -> String {
    Kind::Other.as_str().to_string()
}

/// Canonical `kind` values. Parsing accepts any string —
/// unknown kinds round-trip as the raw string on `Location`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Studio,
    Room,
    Storage,
    Venue,
    Home,
    Other,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Studio => "studio",
            Self::Room => "room",
            Self::Storage => "storage",
            Self::Venue => "venue",
            Self::Home => "home",
            Self::Other => "other",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "studio" => Some(Self::Studio),
            "room" => Some(Self::Room),
            "storage" | "store" => Some(Self::Storage),
            "venue" => Some(Self::Venue),
            "home" => Some(Self::Home),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Location {
    type Key = uuid::Uuid;
    fn key(&self) -> uuid::Uuid {
        self.id
    }
}
