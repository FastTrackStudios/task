//! `vault::VaultPage` → `Routine` / `WorkoutSession`.
//! Routines: `type: routine`. Sessions: `type: workout`.
//!
//! The field mappings live in [`crate::entity`]; this module keeps the
//! historical `workouts::parse::*` paths working.

pub use vault_entity::ParseError;

use vault::VaultPage;
use vault_entity::store::VaultEntity;

use crate::entity::{Routines, Sessions};
use crate::model::{Routine, WorkoutSession};

/// True when `page` carries `type: routine` (or the tag).
pub fn looks_like_routine(page: &VaultPage) -> bool {
    Routines::matches(page)
}

/// True when `page` carries `type: workout` (or the tag).
pub fn looks_like_session(page: &VaultPage) -> bool {
    Sessions::matches(page)
}

/// Parse a routine page.
pub fn parse_routine(page: &VaultPage) -> Result<Routine, ParseError> {
    Routines::from_page(page)
}

/// Parse a workout-session page.
pub fn parse_session(page: &VaultPage) -> Result<WorkoutSession, ParseError> {
    Sessions::from_page(page)
}
