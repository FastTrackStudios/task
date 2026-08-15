//! `Exercise`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the whole CRUD store — comes from `vault-entity`.
//! What stays here is the part that is genuinely about the exercise
//! catalog: which frontmatter keys map to which fields.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::{Exercise, StringList};

/// Vault mapping marker for [`Exercise`].
pub struct Exercises;

impl VaultEntity for Exercises {
    type Model = Exercise;

    const TYPE: &'static str = "exercise";
    /// Exercises live under the wiki tree so the wiki feature picks
    /// them up like any other curated page.
    const DEFAULT_FOLDER: &'static str = "Wiki/Exercises";

    fn id(m: &Exercise) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Exercise, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Exercise) -> &str {
        &m.path
    }
    fn set_path(m: &mut Exercise, path: String) {
        m.path = path;
    }
    fn name(m: &Exercise) -> &str {
        &m.name
    }

    fn on_create(m: &mut Exercise, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut Exercise, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<Exercise, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;

        // A page with no `id:` gets a stable one derived from its path,
        // so hand-authored files keep the same identity across reads.
        let id = yaml::str_at(&map, "id")
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));

        let tags = yaml::string_list_at(&map, "tags")
            .into_iter()
            .filter(|t| t != Self::TYPE)
            .collect();

        Ok(Exercise {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            aliases: StringList(yaml::string_list_at(&map, "aliases")),
            description: yaml::str_at(&map, "description"),
            category: yaml::str_at(&map, "category").unwrap_or_else(|| "other".into()),
            primary_muscles: StringList(yaml::string_list_at(&map, "primaryMuscles")),
            secondary_muscles: StringList(yaml::string_list_at(&map, "secondaryMuscles")),
            equipment: StringList(yaml::string_list_at(&map, "equipment")),
            mechanics: yaml::str_at(&map, "mechanics"),
            force: yaml::str_at(&map, "force"),
            instructions: StringList(yaml::string_list_at(&map, "instructions")),
            video_url: yaml::str_at(&map, "videoUrl"),
            image_url: yaml::str_at(&map, "imageUrl"),
            tags: StringList(tags),
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            details: body.to_string(),
        })
    }

    fn to_markdown(m: &Exercise) -> Result<String, WriteError> {
        // `path` + `details` are `#[serde(skip)]` on the model, so the
        // whole struct serializes into frontmatter and `details` is
        // appended as the markdown body.
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}
