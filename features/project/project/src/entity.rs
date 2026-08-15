//! `ProjectInfo`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the markdown emitter — comes from `vault-entity`.
//! What stays here is the part that is genuinely about projects: which
//! frontmatter keys map to which fields.
//!
//! Discriminator: `type: project` in the frontmatter, or `project` in
//! `tags:`. Both are matched **case-insensitively** here — vaults in
//! the wild carry `type: Project` — so [`VaultEntity::matches`] is
//! overridden rather than taking the shared exact-match default.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::ProjectInfo;

/// Vault mapping marker for [`ProjectInfo`].
pub struct Projects;

impl VaultEntity for Projects {
    type Model = ProjectInfo;

    const TYPE: &'static str = "project";
    const DEFAULT_FOLDER: &'static str = "Projects";
    /// Not the `TYPE` default: a title that slugifies to nothing has
    /// always produced `untitled-project.md`, and that filename is what
    /// existing vaults contain.
    const SLUG_FALLBACK: &'static str = "untitled-project";

    fn id(p: &ProjectInfo) -> Uuid {
        p.id
    }
    fn set_id(p: &mut ProjectInfo, id: Uuid) {
        p.id = id;
    }
    fn path(p: &ProjectInfo) -> &str {
        &p.path
    }
    fn set_path(p: &mut ProjectInfo, path: String) {
        p.path = path;
    }
    fn name(p: &ProjectInfo) -> &str {
        &p.title
    }

    fn on_create(p: &mut ProjectInfo, now: DateTime<Utc>) {
        p.date_created.get_or_insert(now);
    }

    fn on_update(p: &mut ProjectInfo, now: DateTime<Utc>) {
        p.date_modified = Some(now);
    }

    /// Case-insensitive on both `type:` and the `project` tag — the
    /// shared default is exact, and existing vaults carry
    /// `type: Project`.
    fn matches(page: &VaultPage) -> bool {
        matches_raw(&page.raw)
    }

    fn from_page(page: &VaultPage) -> Result<ProjectInfo, ParseError> {
        from_parts(&page.rel_path, &page.basename, &page.raw)
    }

    fn to_markdown(p: &ProjectInfo) -> Result<String, WriteError> {
        // Make sure `id` is non-nil before serializing so the file is
        // downstream-FK-safe on first write.
        let mut owned = p.clone();
        if owned.id.is_nil() {
            owned.id = Uuid::new_v4();
        }
        // `path` and `details` are `#[serde(skip)]` on the model, so
        // serializing the whole `ProjectInfo` yields exactly the
        // frontmatter keys; `details` becomes the markdown body.
        let details = owned.details.clone();
        frontmatter::document(Self::TYPE, &owned, &details)
    }
}

/// The discriminator, over raw markdown. Two shapes accepted:
///
/// - `type: project` in the frontmatter, or
/// - `tags: [..., project]` (case-insensitive on `project`).
pub(crate) fn matches_raw(raw: &str) -> bool {
    let Some((map, _)) = frontmatter::mapping(raw) else {
        return false;
    };
    if yaml::str_at(&map, "type").is_some_and(|t| t.eq_ignore_ascii_case(Projects::TYPE)) {
        return true;
    }
    yaml::string_list_at(&map, "tags")
        .iter()
        .any(|t| t.eq_ignore_ascii_case(Projects::TYPE))
}

/// The field mapping, over the raw page parts rather than a
/// `VaultPage` — the lower-level surface `parse::parse_str` exposes
/// for callers holding bytes but no vault.
pub(crate) fn from_parts(
    rel_path: &str,
    basename: &str,
    raw: &str,
) -> Result<ProjectInfo, ParseError> {
    let (map, body) = frontmatter::mapping(raw).ok_or(ParseError::NoFrontmatter)?;

    // `id` is required for stable cross-feature references. When the
    // frontmatter lacks one, derive a deterministic namespace uuid
    // from the path — the same v5 fallback the task / milestone /
    // goal / workstream parsers use — so the same file resolves to
    // the same id on every scan (and across machines) until a save
    // persists it to disk. A random per-scan id here broke
    // `get(list()[i].id)` round trips, deep links, and
    // `task.project_id` pointers.
    let id = yaml::str_at(&map, "id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, rel_path.as_bytes()));

    // Federation pointer — accept both serde-renamed `sameAs`
    // and snake_case for hand-written frontmatter.
    let same_as = yaml::str_at(&map, "sameAs").or_else(|| yaml::str_at(&map, "same_as"));
    let parent_id = yaml::str_at(&map, "parentId")
        .or_else(|| yaml::str_at(&map, "parent_id"))
        .and_then(|s| Uuid::parse_str(&s).ok());
    let project_type = yaml::str_at(&map, "projectType")
        .or_else(|| yaml::str_at(&map, "project_type"))
        .unwrap_or_default();

    Ok(ProjectInfo {
        path: rel_path.to_string(),
        id,
        title: yaml::str_at(&map, "title").unwrap_or_else(|| basename.to_string()),
        status: yaml::str_at(&map, "status").unwrap_or_else(|| "active".into()),
        priority: yaml::str_at(&map, "priority").unwrap_or_else(|| "normal".into()),
        project_type,
        lead: yaml::str_at(&map, "lead").unwrap_or_default(),
        tags: crate::model::Tags(yaml::string_list_at(&map, "tags")),
        same_as,
        target_date: yaml::str_at(&map, "targetDate").and_then(|s| s.parse().ok()),
        progress_percent: yaml::i64_at(&map, "progressPercent")
            .and_then(|n| i16::try_from(n).ok())
            .unwrap_or(-1),
        parent_id,
        details: body.to_string(),
        client_id: yaml::str_at(&map, "clientId").and_then(|s| Uuid::parse_str(&s).ok()),
        billable_default: yaml::bool_at(&map, "billableDefault").unwrap_or(false),
        currency: yaml::str_at(&map, "currency").unwrap_or_default(),
        default_rate_cents: yaml::i64_at(&map, "defaultRateCents").unwrap_or(0),
        estimated_seconds: yaml::i64_at(&map, "estimatedSeconds").unwrap_or(0),
        agent_profile: yaml::str_at(&map, "agentProfile").unwrap_or_default(),
        verify_command: yaml::str_at(&map, "verifyCommand").unwrap_or_default(),
        color: yaml::str_at(&map, "color").unwrap_or_default(),
        image: yaml::str_at(&map, "image").unwrap_or_default(),
        archived: yaml::bool_at(&map, "archived").unwrap_or(false),
        states: take_states(&map),
        date_created: yaml::timestamp_at(&map, "dateCreated"),
        date_modified: yaml::timestamp_at(&map, "dateModified"),
    })
}

/// Parse the optional `states:` registry. Tolerant: an unparseable or
/// empty list reads as `None` (treated as "use the default registry")
/// rather than failing the page.
fn take_states(map: &serde_yaml::Mapping) -> Option<crate::states::StatesConfig> {
    let value = map.get("states")?;
    let cfg: crate::states::StatesConfig = serde_yaml::from_value(value.clone()).ok()?;
    (!cfg.is_empty()).then_some(cfg)
}
