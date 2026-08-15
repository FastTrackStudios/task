//! `Location` → markdown bytes + path helpers.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `locations::write::*` paths working and adds the one
//! thing the shared store doesn't cover — writing a location straight
//! to a vault root on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::Locations;
use crate::model::Location;

/// Render a location as a full markdown page (`type: location`
/// frontmatter plus the `details` body). Empty optional fields are
/// skipped to keep new files terse.
pub fn serialize_location(loc: &Location) -> Result<String, WriteError> {
    Locations::to_markdown(loc)
}

/// Default layout: `Operations/Locations/<slug>.md`.
#[must_use]
pub fn default_location_path(name: &str, folder: Option<&str>) -> String {
    Locations::default_path(name, folder)
}

/// Write `loc` to `<vault_root>/<loc.path>`, creating parent
/// directories.
pub fn write_location(
    vault_root: &Path,
    loc: &mut Location,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if loc.path.is_empty() {
        return Err(WriteError::BadPath("location.path is empty".into()));
    }
    let abs = vault_root.join(&loc.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    Locations::on_create(loc, now);
    Locations::on_update(loc, now);
    let body = serialize_location(loc)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}
