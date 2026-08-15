//! `Item`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the whole CRUD store — comes from `vault-entity`.
//! What stays here is the part that is genuinely about inventory:
//! which frontmatter keys map to which fields.
//!
//! Discriminator: `type: item` in the frontmatter, or `item` in
//! `tags:`. Missing optional fields fall back to defaults; a missing
//! `id` is synthesized from the page path so legacy pages still load
//! with a stable identity — callers should `write_item` to persist a
//! real uuid.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::{Item, StringList};

/// Vault mapping marker for [`Item`].
pub struct Items;

impl VaultEntity for Items {
    type Model = Item;

    const TYPE: &'static str = "item";
    const DEFAULT_FOLDER: &'static str = "Operations/Inventory";

    fn id(m: &Item) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Item, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Item) -> &str {
        &m.path
    }
    fn set_path(m: &mut Item, path: String) {
        m.path = path;
    }
    fn name(m: &Item) -> &str {
        &m.name
    }

    fn on_create(m: &mut Item, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut Item, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<Item, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;

        // A page with no `id:` gets a stable one derived from its path,
        // so hand-authored files keep the same identity across reads.
        let id = yaml::str_at(&map, "id")
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));

        // The discriminator tag is structural, not user data — drop it
        // so a round-trip doesn't duplicate it.
        let tags: Vec<String> = yaml::string_list_at(&map, "tags")
            .into_iter()
            .filter(|t| t != Self::TYPE)
            .collect();

        Ok(Item {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            category: yaml::str_at(&map, "category").unwrap_or_default(),
            location_id: yaml::str_at(&map, "location_id").and_then(|s| Uuid::parse_str(&s).ok()),
            condition: yaml::str_at(&map, "condition").unwrap_or_else(|| "good".into()),
            status: yaml::str_at(&map, "status").unwrap_or_else(|| "stored".into()),
            manufacturer: yaml::str_at(&map, "manufacturer"),
            model: yaml::str_at(&map, "model"),
            serial: yaml::str_at(&map, "serial"),
            purchase_date: yaml::date_at(&map, "purchaseDate"),
            value: yaml::f64_at(&map, "value"),
            tasks: StringList(yaml::string_list_at(&map, "tasks")),
            tags: StringList(tags),
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            details: body.to_string(),
        })
    }

    fn to_markdown(m: &Item) -> Result<String, WriteError> {
        // `path` and `details` are `#[serde(skip)]` on the model, so
        // serializing the whole `Item` yields exactly the frontmatter
        // keys; `details` becomes the markdown body. Empty optionals
        // are `skip_serializing_if`'d so new files stay terse and
        // diffs on status flips stay small.
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}
