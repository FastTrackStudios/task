//! `IntakeLog`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the whole CRUD store — comes from `vault-entity`.
//! What stays here is the part that is genuinely about the intake log:
//! which frontmatter keys map to which fields, and how an entry row
//! decodes.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::{IntakeEntry, IntakeLog, IntakeSource};
use mealplan::cookbook::Nutrition;

/// Vault mapping marker for [`IntakeLog`].
pub struct IntakeLogs;

impl VaultEntity for IntakeLogs {
    type Model = IntakeLog;

    const TYPE: &'static str = "intake-log";
    /// One page per day; the real filename is the ISO date, not a
    /// name slug — see [`crate::write::default_intake_path`], which
    /// [`crate::store::Store`] applies before delegating to the
    /// shared store.
    const DEFAULT_FOLDER: &'static str = "intake";

    fn id(m: &IntakeLog) -> Uuid {
        m.id
    }
    fn set_id(m: &mut IntakeLog, id: Uuid) {
        m.id = id;
    }
    fn path(m: &IntakeLog) -> &str {
        &m.path
    }
    fn set_path(m: &mut IntakeLog, path: String) {
        m.path = path;
    }
    fn name(m: &IntakeLog) -> &str {
        &m.name
    }

    fn on_create(m: &mut IntakeLog, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut IntakeLog, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<IntakeLog, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;

        // A page with no `id:` gets a stable one derived from its path,
        // so hand-authored files keep the same identity across reads.
        let id = yaml::str_at(&map, "id")
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()));

        let date = yaml::date_at(&map, "date")
            .ok_or_else(|| ParseError::Field("missing required field: date".into()))?;

        let target = map
            .get("target")
            .and_then(|v| serde_yaml::from_value::<Nutrition>(v.clone()).ok());

        let tags = yaml::string_list_at(&map, "tags")
            .into_iter()
            .filter(|t| t != Self::TYPE)
            .collect();

        Ok(IntakeLog {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            date,
            entries: crate::model::Entries(parse_entries(&map)),
            target: crate::model::DailyTarget(target),
            tags: crate::model::Tags(tags),
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            details: body.to_string(),
        })
    }

    fn to_markdown(m: &IntakeLog) -> Result<String, WriteError> {
        // `path` + `details` are `#[serde(skip)]` on the model, so the
        // whole struct serializes into frontmatter and `details` is
        // appended as the markdown body.
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}

fn parse_entries(map: &serde_yaml::Mapping) -> Vec<IntakeEntry> {
    let Some(seq) = map.get("entries").and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|row| {
            let m = row.as_mapping()?;
            Some(IntakeEntry {
                id: yaml::str_at(m, "id")
                    .and_then(|s| Uuid::parse_str(&s).ok())
                    .unwrap_or_else(Uuid::new_v4),
                source: parse_source(m.get("source")?)?,
                name: yaml::str_at(m, "name")?,
                qty: yaml::f64_at(m, "qty")?,
                unit: yaml::str_at(m, "unit").unwrap_or_default(),
                time: yaml::str_at(m, "time").and_then(|s| s.parse().ok()),
                slot: yaml::str_at(m, "slot"),
                nutrition: m
                    .get("nutrition")
                    .and_then(|v| serde_yaml::from_value::<Nutrition>(v.clone()).ok()),
                note: yaml::str_at(m, "note"),
            })
        })
        .collect()
}

fn parse_source(v: &serde_yaml::Value) -> Option<IntakeSource> {
    let m = v.as_mapping()?;
    let kind = m.get("kind").and_then(|v| v.as_str())?;
    match kind {
        "recipe" => {
            let path = m
                .get("path")
                .or_else(|| m.get("id")) // legacy alias
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string)?;
            Some(IntakeSource::Recipe { path })
        }
        "pantry" => {
            let id = m
                .get("id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())?;
            Some(IntakeSource::Pantry { id })
        }
        "freeform" => Some(IntakeSource::Freeform),
        _ => None,
    }
}
