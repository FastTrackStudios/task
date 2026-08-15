//! `BodyMetric`'s vault mapping.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the whole CRUD store — comes from `vault-entity`.
//! What stays here is the part that is genuinely about body metrics:
//! which frontmatter keys map to which fields.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::{BodyEntry, BodyMetric};

/// Vault mapping marker for [`BodyMetric`].
pub struct BodyMetrics;

impl VaultEntity for BodyMetrics {
    type Model = BodyMetric;

    const TYPE: &'static str = "body-metric";
    const DEFAULT_FOLDER: &'static str = "Projects/Fitness/body";

    fn id(m: &BodyMetric) -> Uuid {
        m.id
    }
    fn set_id(m: &mut BodyMetric, id: Uuid) {
        m.id = id;
    }
    fn path(m: &BodyMetric) -> &str {
        &m.path
    }
    fn set_path(m: &mut BodyMetric, path: String) {
        m.path = path;
    }
    fn name(m: &BodyMetric) -> &str {
        &m.name
    }

    fn on_create(m: &mut BodyMetric, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut BodyMetric, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<BodyMetric, ParseError> {
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

        Ok(BodyMetric {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            kind: yaml::str_at(&map, "kind").unwrap_or_else(|| "other".into()),
            unit: yaml::str_at(&map, "unit").unwrap_or_default(),
            goal: yaml::f64_at(&map, "goal"),
            tags,
            entries: crate::model::Entries(parse_entries(&map)),
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            details: body.to_string(),
        })
    }

    fn to_markdown(m: &BodyMetric) -> Result<String, WriteError> {
        // Sort entries ascending by date on write so the page reads
        // chronologically in a text editor and chart-builders don't
        // need to re-sort.
        let mut owned = m.clone();
        owned.entries.sort_by_key(|e| e.date);
        let details = owned.details.clone();
        frontmatter::document(Self::TYPE, &owned, &details)
    }
}

fn parse_entries(map: &serde_yaml::Mapping) -> Vec<BodyEntry> {
    let Some(seq) = map.get("entries").and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|row| {
            let m = row.as_mapping()?;
            Some(BodyEntry {
                id: yaml::str_at(m, "id")
                    .and_then(|s| Uuid::parse_str(&s).ok())
                    .unwrap_or_else(Uuid::new_v4),
                date: yaml::str_at(m, "date")?.parse().ok()?,
                value: yaml::f64_at(m, "value")?,
                unit: yaml::str_at(m, "unit"),
                note: yaml::str_at(m, "note"),
            })
        })
        .collect()
}
