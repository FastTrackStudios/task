//! `Workstream`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the markdown emitter — comes from `vault-entity`.
//! What stays here is the part that is genuinely about workstreams:
//! which frontmatter keys map to which fields.
//!
//! Discriminator: `type: workstream` in the frontmatter (or
//! `workstream` in `tags:`). A missing `id` is synthesized from the
//! page path so legacy pages still load with a stable identity;
//! `projectId` is the one genuinely required key.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};
use workflows_proto::AgentRef;

use crate::model::{AgentRefList, Links, Workstream};

/// Vault mapping marker for [`Workstream`].
pub struct Workstreams;

impl VaultEntity for Workstreams {
    type Model = Workstream;

    const TYPE: &'static str = "workstream";
    /// Only a fallback for [`VaultEntity::default_path`], which
    /// workstreams never take: their real layout is derived from the
    /// owning project's folder by
    /// [`crate::write::default_workstream_path`].
    const DEFAULT_FOLDER: &'static str = "Projects";

    fn id(w: &Workstream) -> Uuid {
        w.id
    }
    fn set_id(w: &mut Workstream, id: Uuid) {
        w.id = id;
    }
    fn path(w: &Workstream) -> &str {
        &w.path
    }
    fn set_path(w: &mut Workstream, path: String) {
        w.path = path;
    }
    fn name(w: &Workstream) -> &str {
        &w.title
    }

    fn on_create(w: &mut Workstream, now: DateTime<Utc>) {
        w.date_created.get_or_insert(now);
    }

    fn on_update(w: &mut Workstream, now: DateTime<Utc>) {
        w.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<Workstream, ParseError> {
        from_parts(&page.rel_path, &page.basename, &page.raw)
    }

    fn to_markdown(w: &Workstream) -> Result<String, WriteError> {
        // `path` and `details` are `#[serde(skip)]` on the model, so
        // serializing the whole `Workstream` yields exactly the
        // frontmatter keys; `details` becomes the markdown body.
        frontmatter::document(Self::TYPE, w, &w.details)
    }
}

/// The field mapping, over the raw page parts rather than a
/// `VaultPage` — the lower-level surface `parse::parse_workstream`
/// exposes for callers holding bytes but no vault.
pub(crate) fn from_parts(
    rel_path: &str,
    basename: &str,
    raw: &str,
) -> Result<Workstream, ParseError> {
    let (map, body) = frontmatter::mapping(raw).ok_or(ParseError::NoFrontmatter)?;

    let id = yaml::str_at(&map, "id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, rel_path.as_bytes()));

    // Accept both the JSON-friendly serde renames and their snake_case
    // spellings so hand-written frontmatter doesn't fail silently.
    let project_id = yaml::str_at(&map, "projectId")
        .or_else(|| yaml::str_at(&map, "project_id"))
        .and_then(|s| Uuid::parse_str(&s).ok())
        .ok_or_else(|| ParseError::Field("workstream is missing required `projectId`".into()))?;
    let start_date = yaml::str_at(&map, "startDate")
        .or_else(|| yaml::str_at(&map, "start_date"))
        .and_then(|s| s.parse().ok());
    let target_date = yaml::str_at(&map, "targetDate")
        .or_else(|| yaml::str_at(&map, "target_date"))
        .and_then(|s| s.parse().ok());

    // `lead` / `members` are serde-tagged `AgentRef` mappings —
    // deserialize through serde_yaml rather than field-picking.
    let lead: Option<AgentRef> = map
        .get("lead")
        .and_then(|v| serde_yaml::from_value(v.clone()).ok());
    let members: Vec<AgentRef> = map
        .get("members")
        .and_then(|v| serde_yaml::from_value(v.clone()).ok())
        .unwrap_or_default();

    Ok(Workstream {
        path: rel_path.to_string(),
        id,
        title: yaml::str_at(&map, "title").unwrap_or_else(|| basename.to_string()),
        project_id,
        status: yaml::str_at(&map, "status").unwrap_or_else(|| "backlog".into()),
        lead,
        members: AgentRefList(members),
        start_date,
        target_date,
        links: Links(yaml::string_list_at(&map, "links")),
        date_created: yaml::timestamp_at(&map, "dateCreated"),
        date_modified: yaml::timestamp_at(&map, "dateModified"),
        details: body.to_string(),
    })
}
