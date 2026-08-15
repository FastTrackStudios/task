//! Write `.cook` files.
//!
//! The `.cook` source is the source of truth; the [`Recipe`]
//! struct is a parsed view. Editors mutate
//! [`crate::model::Recipe::source`] and call [`write_cook`]
//! to persist the new text.

use std::path::{Path, PathBuf};

use crate::model::Recipe;
use crate::scan::COOKBOOK_DIR;

pub use vault_entity::WriteError;

/// Default layout: `Cookbook/<slug>.cook`, relative to the
/// wiki root (`<org>/wiki/Knowledge/`). Override `folder` for
/// sub-folders under the cookbook root.
#[must_use]
pub fn default_recipe_path(name: &str, folder: Option<&str>) -> String {
    let slug = vault_entity::slugify(name, "recipe");
    match folder {
        Some(f) => format!("{}/{slug}.cook", f.trim_end_matches('/')),
        None => format!("{COOKBOOK_DIR}/{slug}.cook"),
    }
}

/// Write `recipe.source` to `<vault_root>/<recipe.path>`.
/// Creates parent directories as needed.
pub fn write_cook(
    vault_root: &Path,
    recipe: &Recipe,
    overwrite: bool,
) -> Result<PathBuf, WriteError> {
    if recipe.path.is_empty() {
        return Err(WriteError::BadPath("recipe.path is empty".into()));
    }
    let abs = vault_root.join(&recipe.path);
    if !overwrite && abs.exists() {
        return Err(WriteError::Exists(abs.display().to_string()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    std::fs::write(&abs, &recipe.source).map_err(|e| WriteError::Io(e.to_string()))?;
    Ok(abs)
}

/// Move a `.cook` file plus any sibling step/title images.
pub fn rename_cook(vault_root: &Path, old_path: &str, new_path: &str) -> Result<(), WriteError> {
    let old_abs = vault_root.join(old_path);
    let new_abs = vault_root.join(new_path);
    if !old_abs.exists() {
        return Err(WriteError::BadPath(format!("missing: {old_path}")));
    }
    if new_abs.exists() {
        return Err(WriteError::Exists(new_abs.display().to_string()));
    }
    if let Some(parent) = new_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Io(e.to_string()))?;
    }
    std::fs::rename(&old_abs, &new_abs).map_err(|e| WriteError::Io(e.to_string()))?;
    // Move sibling images that match the old stem.
    if let (Some(old_stem), Some(new_stem)) = (
        std::path::Path::new(old_path)
            .file_stem()
            .and_then(|s| s.to_str()),
        std::path::Path::new(new_path)
            .file_stem()
            .and_then(|s| s.to_str()),
    ) {
        if let Some(parent) = old_abs.parent() {
            if parent.exists() {
                rename_sibling_images(parent, old_stem, new_stem)
                    .map_err(|e| WriteError::Io(e.to_string()))?;
            }
        }
    }
    Ok(())
}

fn rename_sibling_images(dir: &Path, old_stem: &str, new_stem: &str) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if !matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif"
        ) {
            continue;
        }
        let new_file = if file_stem == old_stem {
            Some(format!("{new_stem}.{ext}"))
        } else {
            file_stem
                .strip_prefix(&format!("{old_stem}."))
                .map(|rest| format!("{new_stem}.{rest}.{ext}"))
        };
        if let Some(new_file) = new_file {
            std::fs::rename(&path, dir.join(new_file))?;
        }
    }
    Ok(())
}

/// Delete a `.cook` file (and sibling images).
pub fn delete_cook(vault_root: &Path, recipe_path: &str) -> Result<(), WriteError> {
    let abs = vault_root.join(recipe_path);
    if !abs.exists() {
        return Err(WriteError::BadPath(format!("missing: {recipe_path}")));
    }
    std::fs::remove_file(&abs).map_err(|e| WriteError::Io(e.to_string()))?;
    if let (Some(parent), Some(stem)) = (
        abs.parent(),
        std::path::Path::new(recipe_path)
            .file_stem()
            .and_then(|s| s.to_str()),
    ) {
        let _ = remove_sibling_images(parent, stem);
    }
    Ok(())
}

fn remove_sibling_images(dir: &Path, stem: &str) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if !matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif"
        ) {
            continue;
        }
        if file_stem == stem || file_stem.starts_with(&format!("{stem}.")) {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}
