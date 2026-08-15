// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `cookbook` — cooklang-native recipe storage.
//!
//! Recipes are `.cook` files under `<wiki>/Cookbook/` parsed
//! by the [`cooklang`](https://cooklang.org) crate. One file
//! per recipe, no sidecar; identity is the vault-relative path.
//!
//! Ingredient names ARE wikilink targets — the wiki layer
//! resolves `@flour` to a `[[flour]]` page (e.g. a pantry item
//! holding nutrition data) so the recipe knows nothing about
//! pantry IDs. Mealprep matches at runtime by name.
//!
//! Surface:
//! - [`Recipe`] / [`Ingredient`] / [`Course`]
//! - [`parse_cook`] / [`parse_cook_at`]
//! - [`scan_cookbook`] / [`image_paths_for`] / [`RecipeImage`]
//! - [`write_cook`] / [`rename_cook`] / [`delete_cook`] /
//!   [`default_recipe_path`]
//! - [`CookbookService`] — `#[architect::rpc]` trait
//! - [`Store`] — file-backed impl. Cheap to `Clone`.

#![cfg(not(target_arch = "wasm32"))]

pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod units;
pub mod wiki;
pub mod write;

pub use model::{
    Course, Ingredient, Ingredients, Nutrition, Recipe, RecipeImages, StepCookware, StepIngredient,
    StepLink, StringList,
};
pub use parse::{ParseError, parse_cook, parse_cook_at};
pub use scan::{COOKBOOK_DIR, RecipeImage, image_paths_for, scan_cookbook, scan_cookbook_at};
pub use service::{CookbookError, CookbookService, CookbookServiceRpc};
pub use store::Store;
pub use units::{compatible, convert};

#[cfg(feature = "vox")]
pub use service::{
    CookbookServiceClient, CookbookServiceRpcDispatcher as CookbookDispatcher,
    Service as CookbookServiceBridge,
    cookbook_service_rpc_service_descriptor as cookbook_service_descriptor,
    layer as cookbook_service_layer, serve as serve_cookbook_service,
};
pub use wiki::{WikiEdge, WikiEdgeKind, recipe_wiki_edges};
pub use write::{WriteError, default_recipe_path, delete_cook, rename_cook, write_cook};
