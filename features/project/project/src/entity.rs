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
        // The migration, and it happens on save rather than on read:
        // whatever the page carried, what goes back is `capabilities`.
        // `projectType` is dropped once its meaning has been carried
        // across, so a page has one field for this and not two — which
        // is what `project.definition.single` is about, and the reason
        // the legacy field is read-only everywhere else.
        //
        // Dropped only when something was carried across. A type nobody
        // could interpret is left exactly as written: deleting it would
        // destroy the only record of what its author meant, on a save
        // that had nothing to do with it.
        // t[impl project.capability.mutable] — capabilities are whatever
        // the page now says. Adding or removing one rewrites this field
        // and nothing else, so no content moves either way
        if !owned.capabilities.held.is_empty() {
            owned.project_type = String::new();
        }
        // Parts get real ids before they reach disk, for the reason in
        // `project_proto::parts`: everything that points at a part
        // points at its id, and an id assigned later is an id every
        // pointer predates.
        for part in &mut owned.parts.0 {
            if part.id.is_nil() {
                part.id = Uuid::new_v4();
            }
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

    // Cloned before the struct takes ownership: `capabilities` is read
    // *through* the legacy field, so both need it.
    let project_type_for_caps = project_type.clone();

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
        parts: take_parts(&map),
        capabilities: take_capabilities(&map, &project_type_for_caps),
        states: take_states(&map),
        date_created: yaml::timestamp_at(&map, "dateCreated"),
        date_modified: yaml::timestamp_at(&map, "dateModified"),
    })
}

/// Parse the optional `parts:` list.
///
/// Tolerant, like `states:`: a malformed list reads as no parts rather
/// than failing the page, because a project whose parts we cannot read
/// is still a project and `vault.index.tolerant` says a parse failure
/// costs one page, not the vault.
///
/// A part with no id gets a deterministic one, derived from the
/// project's id and the part's name, so a hand-written `parts:` list
/// resolves to the same ids on every scan and on every machine — the
/// same v5 trick the page's own id fallback uses, and for the same
/// reason: things point at parts, and a per-scan id breaks every
/// pointer. The next save persists it.
fn take_parts(map: &serde_yaml::Mapping) -> project_proto::Parts {
    let Some(value) = map.get("parts") else {
        return project_proto::Parts::default();
    };
    // Two spellings, because a human writing this by hand writes the
    // short one: `parts: [Overture, Daybreak]` as well as the full
    // `- id: … / name: …` form.
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Written {
        Named(String),
        Full {
            #[serde(default)]
            id: Option<Uuid>,
            name: String,
        },
    }
    let Ok(written) = serde_yaml::from_value::<Vec<Written>>(value.clone()) else {
        return project_proto::Parts::default();
    };
    let project_id = yaml::str_at(map, "id").unwrap_or_default();
    let mut parts = Vec::with_capacity(written.len());
    for entry in written {
        let (id, name) = match entry {
            Written::Named(name) => (None, name),
            Written::Full { id, name } => (id, name),
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            continue;
        }
        let id = id.unwrap_or_else(|| {
            Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("{project_id}/parts/{name}").as_bytes(),
            )
        });
        parts.push(project_proto::Part { id, name });
    }
    project_proto::Parts(parts)
}

// t[impl project.identity.declaration] — capabilities are read from the
// project's own frontmatter and nowhere else, so nothing outside that
// file is needed to interpret it
/// Parse `capabilities:`, falling back to the legacy `projectType`.
///
/// The compatibility path described in `project_proto::parts`: a page
/// that declares capabilities is read as written, and a page carrying
/// only the old free-string type is read through it. A page with both
/// is read from `capabilities` — it is the field this code writes, so
/// it is the one that was most recently meant.
fn take_capabilities(map: &serde_yaml::Mapping, project_type: &str) -> project_proto::Capabilities {
    if let Some(value) = map.get("capabilities") {
        // A single string is accepted as a set of one, because
        // `capabilities: music-production` is what a person writes.
        let names = match value {
            serde_yaml::Value::String(one) => vec![one.clone()],
            other => serde_yaml::from_value::<Vec<String>>(other.clone()).unwrap_or_default(),
        };
        if !names.is_empty() {
            return project_proto::Capabilities::from_names(names);
        }
    }
    project_proto::Capabilities::from_project_type(project_type)
}

/// Parse the optional `states:` registry. Tolerant: an unparseable or
/// empty list reads as `None` (treated as "use the default registry")
/// rather than failing the page.
fn take_states(map: &serde_yaml::Mapping) -> Option<crate::states::StatesConfig> {
    let value = map.get("states")?;
    let cfg: crate::states::StatesConfig = serde_yaml::from_value(value.clone()).ok()?;
    (!cfg.is_empty()).then_some(cfg)
}
