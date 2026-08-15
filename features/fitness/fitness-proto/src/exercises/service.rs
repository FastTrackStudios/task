//! `ExercisesService` — wire surface for browsing +
//! mutating the exercise catalog. Same shape as
//! `CookbookService` (also wiki-superset).

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::model::Exercise;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum ExercisesError {
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
pub trait ExercisesService {
    fn list(&self) -> Result<Vec<Exercise>, ExercisesError>;

    fn get(&self, id: &str) -> Result<Exercise, ExercisesError>;

    /// Match by name OR any alias (case-insensitive).
    /// Useful for routine-builder UIs where the user types
    /// "BB Bench" and the catalog resolves to the
    /// canonical "Bench Press" page.
    fn find_by_name(&self, name: &str) -> Result<Exercise, ExercisesError>;

    fn create(&self, exercise: Exercise) -> Result<Exercise, ExercisesError>;

    fn update(&self, exercise: Exercise) -> Result<Exercise, ExercisesError>;

    fn rename(&self, id: &str, new_path: &str) -> Result<Exercise, ExercisesError>;

    fn delete(&self, id: &str) -> Result<(), ExercisesError>;
}
