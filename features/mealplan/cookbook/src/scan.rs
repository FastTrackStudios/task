//! Walk a wiki root for `.cook` files and parse them.
//!
//! The cookbook is a wiki sub-tree: `<wiki>/Cookbook/`. The
//! wiki itself lives at `<org>/wiki/Knowledge/` server-side
//! (see `OrgRoot::wiki_knowledge_dir`), so recipes end up at
//! `<org>/wiki/Knowledge/Cookbook/<slug>.cook`. This module
//! knows nothing about `vault::Vault` or `OrgRoot`; it walks
//! the filesystem directly with `walkdir`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use walkdir::WalkDir;

use crate::model::Recipe;
use crate::parse::parse_cook_at;

/// Default cookbook sub-directory inside the wiki root.
/// Recipes live alongside other wiki content so wikilinks to
/// ingredient pages (`[[saute]]`, `[[mise en place]]`) resolve
/// in the same namespace.
pub const COOKBOOK_DIR: &str = "Cookbook";

/// Walk `<wiki_root>/Cookbook/` and parse every `.cook` file
/// into a [`Recipe`]. Files that fail to parse are skipped
/// with a warning.
#[must_use]
pub fn scan_cookbook(wiki_root: &Path) -> Vec<Recipe> {
    scan_cookbook_at(wiki_root, COOKBOOK_DIR)
}

/// Like [`scan_cookbook`] but scans a custom sub-directory
/// relative to the wiki root.
#[must_use]
pub fn scan_cookbook_at(wiki_root: &Path, sub_dir: &str) -> Vec<Recipe> {
    let root = wiki_root.join(sub_dir);
    if !root.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("cook"))
        })
    {
        let Ok(rel) = entry.path().strip_prefix(wiki_root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let Ok(src) = std::fs::read_to_string(entry.path()) else {
            tracing::warn!(path = %entry.path().display(), "cookbook scan: read failed");
            continue;
        };
        let mtime = std::fs::metadata(entry.path())
            .and_then(|m| m.modified())
            .ok()
            .map(systemtime_to_chrono);
        match parse_cook_at(&rel_str, &src, mtime) {
            Ok(r) => out.push(r),
            Err(e) => {
                tracing::warn!(path = %rel_str, ?e, "cookbook parse failed");
            }
        }
    }
    out
}

/// Discover step + title images for a recipe, following the
/// cooklang/cooklang-find convention:
///
/// - `Cookbook/Pasta.jpg` — title image (next to the recipe).
/// - `Cookbook/Pasta.0.jpg` — step 0 image.
/// - `Cookbook/Pasta.3.jpg` — step 3 image.
///
/// Returns wiki-relative paths (forward-slash separated)
/// suitable for embedding in UI surfaces.
#[must_use]
pub fn image_paths_for(wiki_root: &Path, recipe_path: &str) -> Vec<RecipeImage> {
    let Some(stem) = std::path::Path::new(recipe_path)
        .file_stem()
        .and_then(|s| s.to_str())
    else {
        return Vec::new();
    };
    let parent = std::path::Path::new(recipe_path)
        .parent()
        .map_or_else(PathBuf::new, std::path::Path::to_path_buf);
    let abs_parent = wiki_root.join(&parent);
    if !abs_parent.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(&abs_parent)
        .max_depth(1)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let Some(file_stem) = entry.path().file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if !matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "gif"
        ) {
            continue;
        }
        // file_stem like "Pasta" → title; "Pasta.3" → step 3.
        if file_stem == stem {
            out.push(RecipeImage {
                path: rel_from(wiki_root, entry.path()),
                step_index: None,
            });
        } else if let Some(rest) = file_stem.strip_prefix(&format!("{stem}.")) {
            if let Ok(idx) = rest.parse::<usize>() {
                out.push(RecipeImage {
                    path: rel_from(wiki_root, entry.path()),
                    step_index: Some(idx),
                });
            }
        }
    }
    out.sort_by_key(|i| i.step_index);
    out
}

/// One image discovered for a recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeImage {
    /// Vault-relative path (forward-slash).
    pub path: String,
    /// `None` for title image; `Some(n)` for step `n` image.
    pub step_index: Option<usize>,
}

fn rel_from(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .unwrap_or(abs)
        .to_string_lossy()
        .replace('\\', "/")
}

fn systemtime_to_chrono(t: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(t)
}
