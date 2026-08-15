//! `Goal`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the markdown emitter — comes from `vault-entity`.
//! What stays here is the part that is genuinely about goals: which
//! frontmatter keys map to which fields.
//!
//! Discriminator: `type: goal` in the frontmatter (or `goal` in
//! `tags:`). Missing optional fields fall back to defaults; a missing
//! `id` is synthesized from the page path so legacy pages still load
//! with a stable identity — callers should `write_goal` to persist a
//! real uuid.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::Goal;

/// Vault mapping marker for [`Goal`].
pub struct Goals;

impl VaultEntity for Goals {
    type Model = Goal;

    const TYPE: &'static str = "goal";
    const DEFAULT_FOLDER: &'static str = "Goals";

    fn id(m: &Goal) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Goal, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Goal) -> &str {
        &m.path
    }
    fn set_path(m: &mut Goal, path: String) {
        m.path = path;
    }
    fn name(m: &Goal) -> &str {
        &m.title
    }

    fn on_create(m: &mut Goal, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut Goal, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<Goal, ParseError> {
        from_parts(&page.rel_path, &page.basename, &page.raw)
    }

    fn to_markdown(m: &Goal) -> Result<String, WriteError> {
        // `path` and `details` are `#[serde(skip)]` on the model, so
        // serializing the whole `Goal` yields exactly the frontmatter
        // keys; `details` becomes the markdown body. Empty optionals
        // are `skip_serializing_if`'d so new files stay terse.
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}

/// The field mapping, over the raw page parts rather than a
/// `VaultPage` — the lower-level surface `parse::parse_goal` exposes
/// for CLI importers and migration scripts that have bytes but no
/// vault.
pub(crate) fn from_parts(rel_path: &str, basename: &str, raw: &str) -> Result<Goal, ParseError> {
    let (map, body) = frontmatter::mapping(raw).ok_or(ParseError::NoFrontmatter)?;

    // A page with no `id:` gets a stable one derived from its path,
    // so hand-authored files keep the same identity across reads.
    let id = yaml::str_at(&map, "id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, rel_path.as_bytes()));

    // Accept both the JSON-friendly serde renames and their snake_case
    // spellings so hand-written frontmatter doesn't fail silently.
    let parent_id = yaml::str_at(&map, "parentId")
        .or_else(|| yaml::str_at(&map, "parent_id"))
        .and_then(|s| Uuid::parse_str(&s).ok());
    let target_date = yaml::str_at(&map, "targetDate")
        .or_else(|| yaml::str_at(&map, "target_date"))
        .and_then(|s| s.parse().ok());
    let cycle_id = yaml::str_at(&map, "cycleId")
        .or_else(|| yaml::str_at(&map, "cycle_id"))
        .and_then(|s| Uuid::parse_str(&s).ok());

    // The discriminator tag is structural, not user data — drop it so
    // a round-trip doesn't duplicate it.
    let tags = yaml::string_list_at(&map, "tags")
        .into_iter()
        .filter(|t| t != Goals::TYPE)
        .collect();

    Ok(Goal {
        path: rel_path.to_string(),
        id,
        title: yaml::str_at(&map, "title").unwrap_or_else(|| basename.to_string()),
        kind: yaml::str_at(&map, "kind").unwrap_or_else(|| "lifetime".into()),
        status: yaml::str_at(&map, "status").unwrap_or_else(|| "aspiration".into()),
        parent_id,
        target_date,
        cycle_id,
        tags,
        date_created: yaml::timestamp_at(&map, "dateCreated"),
        date_modified: yaml::timestamp_at(&map, "dateModified"),
        details: body.to_string(),
    })
}
