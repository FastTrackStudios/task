//! `IntakeLog` → markdown bytes + path helpers.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `intake::write::*` paths working and adds the one thing
//! the shared store doesn't cover — writing a log straight to a vault
//! root on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::IntakeLogs;
use crate::model::IntakeLog;

/// Render a daily log as a full markdown page.
pub fn serialize_intake(log: &IntakeLog) -> Result<String, WriteError> {
    IntakeLogs::to_markdown(log)
}

/// Write `log` to `<vault_root>/<log.path>`, creating parent
/// directories.
pub fn write_intake(
    vault_root: &Path,
    log: &mut IntakeLog,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if log.path.is_empty() {
        return Err(WriteError::BadPath("intake.path is empty".into()));
    }
    let abs = vault_root.join(&log.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    IntakeLogs::on_create(log, now);
    IntakeLogs::on_update(log, now);
    let body = serialize_intake(log)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Default layout: `intake/<YYYY-MM-DD>.md`. One page per
/// day; the date is the filename so directory listings
/// sort chronologically.
///
/// Keyed on the date rather than the log's name, so this can't go
/// through [`VaultEntity::default_path`] — [`crate::store::Store`]
/// applies it before handing the log to the shared store.
pub fn default_intake_path(date: chrono::NaiveDate, folder: Option<&str>) -> String {
    let date_str = date.format("%Y-%m-%d");
    let dir = folder
        .unwrap_or(IntakeLogs::DEFAULT_FOLDER)
        .trim_end_matches('/');
    format!("{dir}/{date_str}.md")
}
