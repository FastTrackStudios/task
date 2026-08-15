//! `Milestone`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the markdown emitter — comes from `vault-entity`.
//! What stays here is the part that is genuinely about milestones:
//! which frontmatter keys map to which fields.
//!
//! Discriminator: `type: milestone` in the frontmatter (or `milestone`
//! in `tags:`). A missing `id` is synthesized from the page path so
//! legacy pages still load with a stable identity; `projectId` is the
//! one genuinely required key — a milestone always belongs to a
//! project.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::Milestone;

/// Vault mapping marker for [`Milestone`].
pub struct Milestones;

impl VaultEntity for Milestones {
    type Model = Milestone;

    const TYPE: &'static str = "milestone";
    /// Only a fallback for [`VaultEntity::default_path`], which
    /// milestones never take: their real layout is derived from the
    /// owning project's folder by
    /// [`crate::write::default_milestone_path`].
    const DEFAULT_FOLDER: &'static str = "Projects";

    fn id(m: &Milestone) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Milestone, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Milestone) -> &str {
        &m.path
    }
    fn set_path(m: &mut Milestone, path: String) {
        m.path = path;
    }
    fn name(m: &Milestone) -> &str {
        &m.title
    }

    fn on_create(m: &mut Milestone, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut Milestone, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<Milestone, ParseError> {
        from_parts(&page.rel_path, &page.basename, &page.raw)
    }

    fn to_markdown(m: &Milestone) -> Result<String, WriteError> {
        // `path` and `details` are `#[serde(skip)]` on the model, so
        // serializing the whole `Milestone` yields exactly the
        // frontmatter keys; `details` becomes the markdown body.
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}

/// The field mapping, over the raw page parts rather than a
/// `VaultPage` — the lower-level surface `parse::parse_milestone`
/// exposes for callers holding bytes but no vault.
pub(crate) fn from_parts(
    rel_path: &str,
    basename: &str,
    raw: &str,
) -> Result<Milestone, ParseError> {
    let (map, body) = frontmatter::mapping(raw).ok_or(ParseError::NoFrontmatter)?;

    let id = yaml::str_at(&map, "id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, rel_path.as_bytes()));

    // Accept both the JSON-friendly serde renames and their snake_case
    // spellings so hand-written frontmatter doesn't fail silently.
    let project_id = yaml::str_at(&map, "projectId")
        .or_else(|| yaml::str_at(&map, "project_id"))
        .and_then(|s| Uuid::parse_str(&s).ok())
        .ok_or_else(|| ParseError::Field("milestone is missing required `projectId`".into()))?;
    let goal_id = yaml::str_at(&map, "goalId")
        .or_else(|| yaml::str_at(&map, "goal_id"))
        .and_then(|s| Uuid::parse_str(&s).ok());
    let due_date = yaml::str_at(&map, "dueDate")
        .or_else(|| yaml::str_at(&map, "due_date"))
        .and_then(|s| s.parse().ok());
    let forge_ref = yaml::str_at(&map, "forgeRef").or_else(|| yaml::str_at(&map, "forge_ref"));

    // The discriminator tag is structural, not user data — drop it so
    // a round-trip doesn't duplicate it.
    let tags = yaml::string_list_at(&map, "tags")
        .into_iter()
        .filter(|t| t != Milestones::TYPE)
        .collect();

    Ok(Milestone {
        path: rel_path.to_string(),
        id,
        title: yaml::str_at(&map, "title").unwrap_or_else(|| basename.to_string()),
        project_id,
        goal_id,
        status: yaml::str_at(&map, "status").unwrap_or_else(|| "open".into()),
        due_date,
        tags,
        forge_ref,
        date_created: yaml::timestamp_at(&map, "dateCreated"),
        date_modified: yaml::timestamp_at(&map, "dateModified"),
        details: body.to_string(),
    })
}
