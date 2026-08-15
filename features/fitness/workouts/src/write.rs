//! Serializers + path helpers for routines + sessions.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `workouts::write::*` paths working and adds the one
//! thing the shared store doesn't cover — writing a page straight to a
//! vault root on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::{Routines, Sessions};
use crate::model::{Routine, WorkoutSession};

/// Render a routine as a full markdown page.
pub fn serialize_routine(r: &Routine) -> Result<String, WriteError> {
    Routines::to_markdown(r)
}

/// Render a session as a full markdown page.
pub fn serialize_session(s: &WorkoutSession) -> Result<String, WriteError> {
    Sessions::to_markdown(s)
}

pub fn write_routine(
    vault_root: &Path,
    r: &mut Routine,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    write_page::<Routines>(vault_root, r, overwrite)
}

pub fn write_session(
    vault_root: &Path,
    s: &mut WorkoutSession,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    write_page::<Sessions>(vault_root, s, overwrite)
}

/// Write `model` to `<vault_root>/<model.path>`, creating parent
/// directories.
///
/// The timestamps are stamped *before* the page is serialized. They used
/// to be stamped after, against an already-rendered body — so the file on
/// disk kept the previous `dateCreated`/`dateModified` (usually absent on
/// a first write) while the caller's struct came back carrying the new
/// ones. Every other slice stamps first; these two were the outliers.
fn write_page<E: VaultEntity>(
    vault_root: &Path,
    model: &mut E::Model,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if E::path(model).is_empty() {
        return Err(WriteError::BadPath("path is empty".into()));
    }
    let abs = vault_root.join(E::path(model));
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    E::on_create(model, now);
    E::on_update(model, now);
    let body = E::to_markdown(model)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Default layout: `routines/<slug>.md`.
pub fn default_routine_path(name: &str, folder: Option<&str>) -> String {
    Routines::default_path(name, folder)
}

/// Default layout: `workouts/<YYYY-MM-DD>-<slug>.md`. Date
/// goes first so directory listings sort chronologically.
///
/// The date prefix means this can't go through
/// [`VaultEntity::default_path`] — [`crate::store::Store`] applies it
/// before handing the session to the shared store.
pub fn default_session_path(date: chrono::NaiveDate, name: &str, folder: Option<&str>) -> String {
    let slug = vault_entity::slugify(name, Sessions::SLUG_FALLBACK);
    let date_str = date.format("%Y-%m-%d");
    let dir = folder
        .unwrap_or(Sessions::DEFAULT_FOLDER)
        .trim_end_matches('/');
    format!("{dir}/{date_str}-{slug}.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine() -> Routine {
        Routine {
            path: "routines/push-pull-legs.md".into(),
            id: uuid::Uuid::new_v4(),
            name: "Push Pull Legs".into(),
            description: None,
            days: Vec::new().into(),
            tags: Vec::new().into(),
            date_created: None,
            date_modified: None,
            details: String::new(),
        }
    }

    /// Read a timestamp key back out of a written page. Compares parsed
    /// instants rather than text — serde_yaml emits the `Z` spelling of
    /// RFC-3339 and `to_rfc3339` emits `+00:00`.
    fn stamp_on_disk(page: &str, key: &str) -> chrono::DateTime<Utc> {
        let (map, _) = vault_entity::frontmatter::mapping(page)
            .unwrap_or_else(|| panic!("page has no frontmatter mapping:\n{page}"));
        vault_entity::yaml::timestamp_at(&map, key)
            .unwrap_or_else(|| panic!("`{key}` missing from the written page:\n{page}"))
    }

    /// The timestamps the caller gets back must be the ones on disk.
    /// They used to diverge: the page was serialized before the stamps
    /// were applied, so a first write produced a file with no
    /// `dateCreated` at all while the struct came back carrying one.
    #[test]
    fn first_write_persists_the_stamps_it_returns() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = routine();
        let abs = write_routine(tmp.path(), &mut r, false).unwrap();

        let created = r.date_created.expect("dateCreated stamped on the struct");
        let modified = r.date_modified.expect("dateModified stamped on the struct");

        let page = std::fs::read_to_string(&abs).unwrap();
        assert_eq!(stamp_on_disk(&page, "dateCreated"), created);
        assert_eq!(stamp_on_disk(&page, "dateModified"), modified);
    }

    /// A rewrite keeps the original creation time and advances the
    /// modification time, on disk as well as in the struct.
    #[test]
    fn rewrite_keeps_created_and_advances_modified() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = routine();
        write_routine(tmp.path(), &mut r, false).unwrap();
        let first_created = r.date_created.unwrap();
        let first_modified = r.date_modified.unwrap();

        let abs = write_routine(tmp.path(), &mut r, true).unwrap();
        assert_eq!(r.date_created, Some(first_created), "creation time moved");
        assert!(r.date_modified.unwrap() >= first_modified);

        let page = std::fs::read_to_string(&abs).unwrap();
        assert_eq!(stamp_on_disk(&page, "dateCreated"), first_created);
        assert_eq!(
            stamp_on_disk(&page, "dateModified"),
            r.date_modified.unwrap()
        );
    }
}
