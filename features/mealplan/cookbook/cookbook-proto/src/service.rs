//! `CookbookService` — wire surface for browsing + mutating
//! cooklang `.cook` files.
//!
//! Identity is the vault-relative `path`. There is no UUID;
//! cooklang files don't carry one and the cookbook layer
//! doesn't synthesize one. Meals, agents, and the wiki all
//! reference recipes by path.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::Recipe;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum CookbookError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait CookbookService {
    /// Every `.cook` file currently under `<wiki>/Cookbook/`.
    fn list(&self) -> Result<Vec<Recipe>, CookbookError>;

    /// One recipe by vault-relative path.
    fn get(&self, path: &str) -> Result<Recipe, CookbookError>;

    /// Create a new recipe. Caller supplies `path` + `source`
    /// in [`Recipe`]; backend fills out parsed fields and
    /// stamps `date_modified`.
    fn create(&self, recipe: Recipe) -> Result<Recipe, CookbookError>;

    /// Replace the file at `recipe.path` with `recipe.source`.
    /// Backend re-parses and returns the refreshed view.
    fn update(&self, recipe: Recipe) -> Result<Recipe, CookbookError>;

    /// Move the backing file (and sibling step/title images)
    /// to a new vault-relative path.
    fn rename(&self, old_path: &str, new_path: &str) -> Result<Recipe, CookbookError>;

    /// Delete the backing file (and sibling images).
    fn delete(&self, path: &str) -> Result<(), CookbookError>;

    /// Import a recipe from a web URL: fetch the page, extract the
    /// recipe (JSON-LD / microdata / heuristics), and synthesize a
    /// cooklang `.cook` draft. Returns the **parsed but unsaved** draft
    /// (with a derived `Cookbook/<name>.cook` path) for review — the
    /// caller saves it via [`CookbookService::create`]. Async: it does
    /// network I/O server-side, so the wasm client just awaits it.
    async fn import(&self, url: String) -> Result<Recipe, CookbookError>;

    /// Raw bytes of one image belonging to the cookbook, addressed by
    /// the wiki-relative path in [`Recipe::images`].
    ///
    /// Served over the existing per-org RPC rather than a new public
    /// HTTP route: the images sit in the wiki beside the recipes, and
    /// an unauthenticated path into that tree is a bigger thing to own
    /// than a method behind the permit system already guarding recipes.
    fn image(&self, path: &str) -> Result<Vec<u8>, CookbookError>;

    /// Write one image into the cookbook, addressed the same way
    /// [`CookbookService::image`] reads it. Overwrites.
    ///
    /// The counterpart to the reader, and it needs the same suspicion:
    /// this puts caller-supplied bytes at a caller-supplied path.
    fn put_image(&self, path: &str, bytes: Vec<u8>) -> Result<(), CookbookError>;
}
