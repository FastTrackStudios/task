//! `Location`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the whole CRUD store — comes from `vault-entity`.
//! What stays here is the part that is genuinely about places: which
//! frontmatter keys map to which fields.
//!
//! Discriminator: `type: location` in the frontmatter (or `location`
//! in `tags:`). Missing optional fields fall back to defaults; a
//! missing `id` is synthesized from the page path so legacy pages
//! still load with a stable identity — callers should `write_location`
//! to persist a real uuid.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::Location;

/// Vault mapping marker for [`Location`].
pub struct Locations;

impl VaultEntity for Locations {
    type Model = Location;

    const TYPE: &'static str = "location";
    const DEFAULT_FOLDER: &'static str = "Operations/Locations";

    fn id(m: &Location) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Location, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Location) -> &str {
        &m.path
    }
    fn set_path(m: &mut Location, path: String) {
        m.path = path;
    }
    fn name(m: &Location) -> &str {
        &m.name
    }

    fn on_create(m: &mut Location, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut Location, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<Location, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;

        // A page with no `id:` gets a stable one derived from its path,
        // so hand-authored files keep the same identity across reads.
        let id = yaml::str_at(&map, "id")
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));

        // The discriminator tag is structural, not user data — drop it
        // so a round-trip doesn't duplicate it.
        let tags = yaml::string_list_at(&map, "tags")
            .into_iter()
            .filter(|t| t != Self::TYPE)
            .collect();

        // Accept both `sameAs` (the serde rename, JSON-friendly) and
        // `same_as` (snake_case) so hand-written frontmatter doesn't
        // fail silently. An empty value is `None` — federation
        // pointers are present-or-absent, not nullable.
        let same_as = yaml::str_at(&map, "sameAs").or_else(|| yaml::str_at(&map, "same_as"));

        Ok(Location {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            kind: yaml::str_at(&map, "kind").unwrap_or_else(|| "other".into()),
            parent_id: yaml::str_at(&map, "parent_id").and_then(|s| Uuid::parse_str(&s).ok()),
            address: yaml::str_at(&map, "address"),
            tags,
            same_as,
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            details: body.to_string(),
        })
    }

    fn to_markdown(m: &Location) -> Result<String, WriteError> {
        // `path` and `details` are `#[serde(skip)]` on the model, so
        // serializing the whole `Location` yields exactly the
        // frontmatter keys; `details` becomes the markdown body. Empty
        // optional fields are `skip_serializing_if`'d to keep new
        // files terse.
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}
