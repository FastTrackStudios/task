//! `Exercise` → markdown bytes + path helpers. Default
//! path lives under `Wiki/Exercises/` so the wiki feature
//! picks the page up like any other curated entry.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `exercises::write::*` paths working and adds the one
//! thing the shared store doesn't cover — writing an exercise straight
//! to a vault root on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::Exercises;
use crate::model::Exercise;

/// Render an exercise as a full markdown page.
pub fn serialize_exercise(ex: &Exercise) -> Result<String, WriteError> {
    Exercises::to_markdown(ex)
}

/// Default layout: `Wiki/Exercises/<slug>.md`.
pub fn default_exercise_path(name: &str, folder: Option<&str>) -> String {
    Exercises::default_path(name, folder)
}

/// Write `ex` to `<vault_root>/<ex.path>`, creating parent directories.
pub fn write_exercise(
    vault_root: &Path,
    ex: &mut Exercise,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if ex.path.is_empty() {
        return Err(WriteError::BadPath("exercise.path is empty".into()));
    }
    let abs = vault_root.join(&ex.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    Exercises::on_create(ex, now);
    Exercises::on_update(ex, now);
    let body = serialize_exercise(ex)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}
