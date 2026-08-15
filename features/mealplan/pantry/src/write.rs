//! `PantryItem` → markdown bytes + path helpers.
//!
//! Frontmatter carries `type: item` and ensures `pantry` is
//! in the tag list, so the page round-trips through both the
//! inventory scanner and ours. Empty optional fields are
//! dropped to keep new files terse.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `pantry::write::*` paths working and adds the one thing
//! the shared store doesn't cover — writing straight to a vault root
//! on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::PantryItems;
use crate::model::PantryItem;

pub fn serialize_pantry_item(item: &PantryItem) -> Result<String, WriteError> {
    PantryItems::to_markdown(item)
}

pub fn write_pantry_item(
    vault_root: &Path,
    item: &mut PantryItem,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if item.path.is_empty() {
        return Err(WriteError::BadPath("pantry item.path is empty".into()));
    }
    let abs = vault_root.join(&item.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    PantryItems::on_create(item, now);
    PantryItems::on_update(item, now);
    let body = serialize_pantry_item(item)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Default layout: `Operations/Inventory/Pantry/<slug>.md`.
/// Pantry is a slice of the household inventory registry — food
/// items the Mealplan project consumes via wikilinks.
#[must_use]
pub fn default_pantry_path(name: &str, folder: Option<&str>) -> String {
    PantryItems::default_path(name, folder)
}
