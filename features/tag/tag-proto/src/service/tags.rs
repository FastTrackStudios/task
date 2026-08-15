//! Tag registry CRUD — the per-org set of icon/color decorations for
//! tag names. A flat CRUD surface (like `inbox_proto::Inbox`): the
//! caller owns the [`Tag`] state, the backend just round-trips it.
//!
//! The registry only *decorates* names — applying a tag to an entity is
//! editing that entity's markdown `tags:` list, not a call here.

use crate::error::TagError;
use crate::tag::Tag;

#[architect::rpc]
pub trait TagService {
    /// Every tag in the org's registry.
    fn list_tags(&self) -> Result<Vec<Tag>, TagError>;

    /// One tag by id.
    fn get_tag(&self, id: &str) -> Result<Tag, TagError>;

    /// Create or replace a tag (keyed by `id`).
    fn upsert_tag(&self, tag: &Tag) -> Result<(), TagError>;

    /// Remove a tag from the registry. The name stays usable as a plain
    /// (undecorated) tag wherever it's already written in markdown.
    fn delete_tag(&self, id: &str) -> Result<(), TagError>;
}
