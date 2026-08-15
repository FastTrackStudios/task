//! Vault mappings for `Routine` and `WorkoutSession`.
//!
//! Everything generic — the frontmatter split, the YAML readers, the
//! slug rule, and the whole CRUD store — comes from `vault-entity`.
//! What stays here is the part that is genuinely about workouts: which
//! frontmatter keys map to which fields, and how the nested day /
//! slot / logged-set rows decode.

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::VaultPage;
use vault_entity::error::{ParseError, WriteError};
use vault_entity::store::VaultEntity;
use vault_entity::{frontmatter, yaml};

use crate::model::{LoggedSet, Routine, RoutineDay, RoutineSlot, WorkoutSession};

/// Vault mapping marker for [`Routine`] — `type: routine`.
pub struct Routines;

/// Vault mapping marker for [`WorkoutSession`] — `type: workout`.
pub struct Sessions;

impl VaultEntity for Routines {
    type Model = Routine;

    const TYPE: &'static str = "routine";
    const DEFAULT_FOLDER: &'static str = "routines";
    /// Both page types shared one `slugify` before the split, so an
    /// unsluggable routine name still lands on `workout`.
    const SLUG_FALLBACK: &'static str = "workout";

    fn id(m: &Routine) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Routine, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Routine) -> &str {
        &m.path
    }
    fn set_path(m: &mut Routine, path: String) {
        m.path = path;
    }
    fn name(m: &Routine) -> &str {
        &m.name
    }

    fn on_create(m: &mut Routine, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut Routine, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<Routine, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;

        let id = stable_id(&map, page);
        let tags = yaml::string_list_at(&map, "tags")
            .into_iter()
            .filter(|t| t != Self::TYPE)
            .collect();

        Ok(Routine {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            description: yaml::str_at(&map, "description"),
            days: crate::model::RoutineDays(parse_days(&map)),
            tags: crate::model::Tags(tags),
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            details: body.to_string(),
        })
    }

    fn to_markdown(m: &Routine) -> Result<String, WriteError> {
        // `path` + `details` are `#[serde(skip)]` on the model, so the
        // whole struct serializes into frontmatter and `details` is
        // appended as the markdown body.
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}

impl VaultEntity for Sessions {
    type Model = WorkoutSession;

    const TYPE: &'static str = "workout";
    const DEFAULT_FOLDER: &'static str = "workouts";

    fn id(m: &WorkoutSession) -> Uuid {
        m.id
    }
    fn set_id(m: &mut WorkoutSession, id: Uuid) {
        m.id = id;
    }
    fn path(m: &WorkoutSession) -> &str {
        &m.path
    }
    fn set_path(m: &mut WorkoutSession, path: String) {
        m.path = path;
    }
    fn name(m: &WorkoutSession) -> &str {
        &m.name
    }

    fn on_create(m: &mut WorkoutSession, now: DateTime<Utc>) {
        m.date_created.get_or_insert(now);
    }

    fn on_update(m: &mut WorkoutSession, now: DateTime<Utc>) {
        m.date_modified = Some(now);
    }

    fn from_page(page: &VaultPage) -> Result<WorkoutSession, ParseError> {
        let (map, body) = frontmatter::mapping(&page.raw).ok_or(ParseError::NoFrontmatter)?;

        let id = stable_id(&map, page);
        let date = yaml::date_at(&map, "date")
            .ok_or_else(|| ParseError::Field("missing required field: date".into()))?;
        let tags = yaml::string_list_at(&map, "tags")
            .into_iter()
            .filter(|t| t != Self::TYPE)
            .collect();

        Ok(WorkoutSession {
            path: page.rel_path.clone(),
            id,
            name: yaml::str_at(&map, "name").unwrap_or_else(|| page.basename.clone()),
            date,
            routine_id: yaml::str_at(&map, "routineId").and_then(|s| Uuid::parse_str(&s).ok()),
            day_name: yaml::str_at(&map, "dayName"),
            logged_sets: crate::model::LoggedSets(parse_logged_sets(&map)),
            status: yaml::str_at(&map, "status").unwrap_or_else(|| "completed".into()),
            duration_minutes: u32_at(&map, "durationMinutes"),
            tags: crate::model::Tags(tags),
            date_created: yaml::timestamp_at(&map, "dateCreated"),
            date_modified: yaml::timestamp_at(&map, "dateModified"),
            details: body.to_string(),
        })
    }

    fn to_markdown(m: &WorkoutSession) -> Result<String, WriteError> {
        frontmatter::document(Self::TYPE, m, &m.details)
    }
}

/// A page with no `id:` gets a stable one derived from its path, so
/// hand-authored files keep the same identity across reads.
fn stable_id(map: &serde_yaml::Mapping, page: &VaultPage) -> Uuid {
    yaml::str_at(map, "id")
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or_else(|| Uuid::new_v5(&Uuid::NAMESPACE_URL, page.rel_path.as_bytes()))
}

fn u32_at(map: &serde_yaml::Mapping, key: &str) -> Option<u32> {
    yaml::i64_at(map, key).and_then(|n| u32::try_from(n).ok())
}

fn parse_days(map: &serde_yaml::Mapping) -> Vec<RoutineDay> {
    let Some(seq) = map.get("days").and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|row| {
            let m = row.as_mapping()?;
            let slots = m
                .get("slots")
                .and_then(|v| v.as_sequence())
                .map(|s| s.iter().filter_map(parse_slot).collect())
                .unwrap_or_default();
            Some(RoutineDay {
                name: yaml::str_at(m, "name")?,
                slots,
                note: yaml::str_at(m, "note"),
            })
        })
        .collect()
}

fn parse_slot(v: &serde_yaml::Value) -> Option<RoutineSlot> {
    let m = v.as_mapping()?;
    Some(RoutineSlot {
        exercise_id: yaml::str_at(m, "exerciseId").and_then(|s| Uuid::parse_str(&s).ok())?,
        exercise_name: yaml::str_at(m, "exerciseName").unwrap_or_default(),
        sets: u32_at(m, "sets"),
        reps: yaml::str_at(m, "reps"),
        weight_kg: yaml::f64_at(m, "weightKg"),
        rir: u32_at(m, "rir"),
        rest_seconds: u32_at(m, "restSeconds"),
        note: yaml::str_at(m, "note"),
    })
}

fn parse_logged_sets(map: &serde_yaml::Mapping) -> Vec<LoggedSet> {
    let Some(seq) = map.get("loggedSets").and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|row| {
            let m = row.as_mapping()?;
            Some(LoggedSet {
                id: yaml::str_at(m, "id")
                    .and_then(|s| Uuid::parse_str(&s).ok())
                    .unwrap_or_else(Uuid::new_v4),
                exercise_id: yaml::str_at(m, "exerciseId")
                    .and_then(|s| Uuid::parse_str(&s).ok())?,
                exercise_name: yaml::str_at(m, "exerciseName").unwrap_or_default(),
                order: u32_at(m, "order").unwrap_or(0),
                reps: u32_at(m, "reps")?,
                weight_kg: yaml::f64_at(m, "weightKg").unwrap_or(0.0),
                rir: u32_at(m, "rir"),
                rpe: yaml::f64_at(m, "rpe"),
                completed: yaml::bool_at(m, "completed").unwrap_or(true),
                note: yaml::str_at(m, "note"),
            })
        })
        .collect()
}
