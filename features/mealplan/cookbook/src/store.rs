//! File-backed [`CookbookService`] impl. Source of truth is
//! `<wiki_root>/Cookbook/*.cook` on disk (the wiki root is
//! typically `<org>/wiki/Knowledge/`).
//!
//! Cheap to `Clone` (one `Arc<PathBuf>` inside). Re-scans the
//! cookbook directory on every `list`. The cookbook is
//! typically <100 recipes so this is fine; switch to a cached
//! snapshot if it ever bites.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model::Recipe;
use crate::parse::parse_cook;
use crate::scan::{image_paths_for, scan_cookbook};
use crate::service::{CookbookError, CookbookService};
use crate::write::{delete_cook, rename_cook, write_cook};

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    vault_root: Arc<PathBuf>,
}

impl Store {
    #[must_use]
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: Arc::new(vault_root.into()),
        }
    }

    #[must_use]
    pub fn vault_root(&self) -> &Path {
        self.vault_root.as_path()
    }

    /// Fill in the pictures sitting beside the recipe file.
    ///
    /// Images aren't declared in the cooklang, so the parser can't know
    /// about them — they're found by name on disk, which is why this
    /// belongs to the store rather than to parsing.
    fn attach_images(&self, recipe: &mut Recipe) {
        recipe.images = image_paths_for(self.vault_root.as_path(), &recipe.path)
            .into_iter()
            .map(|i| crate::model::RecipeImage {
                path: i.path,
                step_index: i.step_index.map(|n| n as u32),
            })
            .collect();
    }

    /// The bytes of one image belonging to the cookbook.
    ///
    /// Refuses anything that isn't an image this recipe convention
    /// produces, and anything that tries to climb out of the cookbook
    /// root — this reads arbitrary paths off disk on behalf of a
    /// caller, so it gets to be paranoid rather than convenient.
    pub fn read_image(&self, rel: &str) -> Result<Vec<u8>, CookbookError> {
        if rel.split(['/', '\\']).any(|c| c == ".." || c.is_empty()) || Path::new(rel).is_absolute()
        {
            return Err(CookbookError::NotFound(rel.to_string()));
        }
        let ok_ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "gif"
                )
            });
        if !ok_ext {
            return Err(CookbookError::NotFound(rel.to_string()));
        }
        let abs = self.vault_root.join(rel);
        // Belt and braces: resolve and confirm it really landed inside.
        let root = self.vault_root.canonicalize().map_err(map_io)?;
        let file = abs
            .canonicalize()
            .map_err(|_| CookbookError::NotFound(rel.to_string()))?;
        if !file.starts_with(&root) {
            return Err(CookbookError::NotFound(rel.to_string()));
        }
        std::fs::read(&file).map_err(map_io)
    }

    /// Write an image into the cookbook, creating the directory if the
    /// recipe's folder doesn't exist yet.
    ///
    /// Same guards as reading, plus a size ceiling: these travel to the
    /// client inlined as data URLs, so an unbounded upload is a way to
    /// make every subsequent read of that recipe painful.
    pub fn write_image(&self, rel: &str, bytes: &[u8]) -> Result<(), CookbookError> {
        const MAX: usize = 8 * 1024 * 1024;
        Self::guard_image_path(rel)?;
        if bytes.len() > MAX {
            return Err(CookbookError::Io(format!(
                "image is {} bytes; the cap is {MAX}",
                bytes.len()
            )));
        }
        let abs = self.vault_root.join(rel);
        // Resolve the *parent*, since the file itself may not exist yet,
        // and confirm it really sits under the cookbook root.
        let root = self.vault_root.canonicalize().map_err(map_io)?;
        let parent = abs
            .parent()
            .ok_or_else(|| CookbookError::NotFound(rel.to_string()))?;
        std::fs::create_dir_all(parent).map_err(map_io)?;
        let parent = parent.canonicalize().map_err(map_io)?;
        if !parent.starts_with(&root) {
            return Err(CookbookError::NotFound(rel.to_string()));
        }
        std::fs::write(parent.join(abs.file_name().unwrap_or_default()), bytes).map_err(map_io)
    }

    /// The path rules both image methods share: no climbing out, no
    /// absolute paths, and pictures only — the recipe sources live in
    /// the same directory and these methods have no business touching
    /// them.
    fn guard_image_path(rel: &str) -> Result<(), CookbookError> {
        if rel.split(['/', '\\']).any(|c| c == ".." || c.is_empty()) || Path::new(rel).is_absolute()
        {
            return Err(CookbookError::NotFound(rel.to_string()));
        }
        let ok_ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "gif"
                )
            });
        if ok_ext {
            Ok(())
        } else {
            Err(CookbookError::NotFound(rel.to_string()))
        }
    }
}

fn map_io(e: impl std::fmt::Display) -> CookbookError {
    CookbookError::Io(e.to_string())
}

impl CookbookService for Store {
    fn list(&self) -> Result<Vec<Recipe>, CookbookError> {
        let mut out = scan_cookbook(self.vault_root.as_path());
        for r in &mut out {
            self.attach_images(r);
        }
        Ok(out)
    }

    fn image(&self, path: &str) -> Result<Vec<u8>, CookbookError> {
        self.read_image(path)
    }

    fn put_image(&self, path: &str, bytes: Vec<u8>) -> Result<(), CookbookError> {
        self.write_image(path, &bytes)
    }

    fn get(&self, path: &str) -> Result<Recipe, CookbookError> {
        let abs = self.vault_root.join(path);
        if !abs.exists() {
            return Err(CookbookError::NotFound(path.to_string()));
        }
        let src = std::fs::read_to_string(&abs).map_err(map_io)?;
        let mtime = std::fs::metadata(&abs)
            .and_then(|m| m.modified())
            .ok()
            .map(chrono::DateTime::<chrono::Utc>::from);
        let mut r = parse_cook(path, &src).map_err(map_io)?;
        r.date_modified = mtime;
        self.attach_images(&mut r);
        Ok(r)
    }

    fn create(&self, recipe: Recipe) -> Result<Recipe, CookbookError> {
        if recipe.path.is_empty() {
            return Err(CookbookError::BadRequest("recipe.path is empty".into()));
        }
        let abs = self.vault_root.join(&recipe.path);
        if abs.exists() {
            return Err(CookbookError::AlreadyExists(recipe.path));
        }
        write_cook(self.vault_root.as_path(), &recipe, false).map_err(map_io)?;
        self.get(&recipe.path)
    }

    fn update(&self, recipe: Recipe) -> Result<Recipe, CookbookError> {
        if recipe.path.is_empty() {
            return Err(CookbookError::BadRequest("recipe.path is empty".into()));
        }
        let abs = self.vault_root.join(&recipe.path);
        if !abs.exists() {
            return Err(CookbookError::NotFound(recipe.path));
        }
        write_cook(self.vault_root.as_path(), &recipe, true).map_err(map_io)?;
        self.get(&recipe.path)
    }

    fn rename(&self, old_path: &str, new_path: &str) -> Result<Recipe, CookbookError> {
        rename_cook(self.vault_root.as_path(), old_path, new_path).map_err(map_io)?;
        self.get(new_path)
    }

    fn delete(&self, path: &str) -> Result<(), CookbookError> {
        delete_cook(self.vault_root.as_path(), path).map_err(map_io)
    }

    async fn import(&self, url: String) -> Result<Recipe, CookbookError> {
        // Fetch + extract + synthesize a `.cook` draft (heuristic — no
        // LLM key required). Parsed but NOT written: the caller reviews
        // and saves via `create`.
        let html = recipe_import::fetch_html(&url)
            .await
            .map_err(|e| CookbookError::BadRequest(format!("fetch: {e}")))?;
        let normalized = recipe_import::extract(&html, &url)
            .map_err(|e| CookbookError::BadRequest(format!("extract: {e}")))?;
        let source = recipe_import::synthesize_heuristic(&normalized);
        let path = crate::write::default_recipe_path(&normalized.name, None);
        parse_cook(&path, &source).map_err(map_io)
    }
}
