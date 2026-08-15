//! The [`Tag`] registry entity.
//!
//! A tag is referenced by **name** from an entity's markdown frontmatter
//! (`tags: [food, work]`) — that's the source of truth and stays
//! editable in raw markdown. This registry row *decorates* a name with a
//! user-chosen [`TagIcon`] and color so the calendar chips / list rows
//! can show it at a glance. A name with no registry row still works as a
//! plain tag; it just renders without an icon.
//!
//! A plain `Facet` struct (not an `architect::Entity`): the backing
//! store is a vault-backed JSON registry, so we don't need the
//! sea-orm / Loro column machinery. Promote to a full entity later if
//! tags ever need DB projection or per-row CRDT sync.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::tag_icon::TagIcon;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct Tag {
    /// Stable id (uuid string). PK in the registry; renaming the tag
    /// keeps the id so decorations survive a rename.
    pub id: String,

    /// Display name — the handle used to apply the tag from markdown.
    /// Matched case-insensitively when resolving a frontmatter tag to
    /// its decoration. Unique per org (not enforced here; the store
    /// upserts by id).
    pub name: String,

    /// The icon shown wherever the tag appears.
    pub icon: TagIcon,

    /// 6-char hex without the leading `#`. `None` → the UI derives a
    /// stable color from the name's hash.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<String>,

    /// Optional human-readable description shown on hover.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,
}

impl Tag {
    /// A fresh tag with the default icon. `id` is a uuid string minted
    /// by the caller so the proto stays clock/RNG-agnostic.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            icon: TagIcon::default(),
            color: None,
            description: None,
        }
    }

    /// Case-insensitive name match — how a markdown frontmatter tag name
    /// resolves to its registry decoration.
    #[must_use]
    pub fn matches_name(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name.trim())
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Tag {
    type Key = String;
    fn key(&self) -> String {
        self.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::Tag;

    #[test]
    fn matches_name_is_case_insensitive_and_trimmed() {
        let t = Tag::new("id-1", "Food");
        assert!(t.matches_name("food"));
        assert!(t.matches_name("  FOOD "));
        assert!(!t.matches_name("foodie"));
    }
}
