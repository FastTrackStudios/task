//! `vault::VaultPage` → `Exercise`. Discriminator
//! `type: exercise` in frontmatter or `exercise` in tags.
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `exercises::parse::*` paths working.

pub use vault_entity::ParseError;

use vault::VaultPage;
use vault_entity::store::VaultEntity;

use crate::entity::Exercises;
use crate::model::Exercise;

/// True when `page` carries `type: exercise` (or the tag).
pub fn looks_like_exercise(page: &VaultPage) -> bool {
    Exercises::matches(page)
}

/// Parse an exercise page.
pub fn parse_page(page: &VaultPage) -> Result<Exercise, ParseError> {
    Exercises::from_page(page)
}
