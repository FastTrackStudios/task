//! `Item` → markdown bytes + path helpers.
//!
//! Serialization lives in [`crate::entity`]; this module keeps the
//! historical `inventory::write::*` paths working and adds the one
//! thing the shared store doesn't cover — writing an item straight to
//! a vault root on disk, without an in-memory `Vault`.

use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_entity::store::VaultEntity;

pub use vault_entity::WriteError;

use crate::entity::Items;
use crate::model::Item;

/// Render an item as a full markdown page (`type: item` frontmatter
/// plus the `details` body). Empty optional fields are skipped.
pub fn serialize_item(item: &Item) -> Result<String, WriteError> {
    Items::to_markdown(item)
}

/// Default layout: `Operations/Inventory/<slug>.md`.
#[must_use]
pub fn default_item_path(name: &str, folder: Option<&str>) -> String {
    Items::default_path(name, folder)
}

/// Write `item` to `<vault_root>/<item.path>`, creating parent
/// directories.
pub fn write_item(
    vault_root: &Path,
    item: &mut Item,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if item.path.is_empty() {
        return Err(WriteError::BadPath("item.path is empty".into()));
    }
    let abs = vault_root.join(&item.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    let now = Utc::now();
    Items::on_create(item, now);
    Items::on_update(item, now);
    let body = serialize_item(item)?;
    std::fs::write(&abs, body).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}
