//! The `Recipe` / `Ingredient` / `Course` models (and the
//! `StringList` / `Ingredients` JSON-column newtypes) now live in
//! the wasm-clean [`cookbook_proto`] crate so the web UI can bind
//! to the wire contract directly. Re-exported here so the existing
//! `cookbook::model::*` paths keep working.
//!
//! [`Nutrition`] also lives in [`cookbook_proto`]; re-exported so
//! the existing `cookbook::model::Nutrition` / `cookbook::Nutrition`
//! paths keep working.

pub use cookbook_proto::model::{
    CookStep, CookSteps, Course, Ingredient, Ingredients, Nutrition, Recipe, RecipeImage,
    RecipeImages, RecipeTimer, StepCookware, StepIngredient, StepLink, StringList,
};
