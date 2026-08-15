//! `vault::VaultPage` → `PantryItem`.
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `pantry::parse::*` paths working.
//!
//! Discriminator: a page is a pantry item when it carries
//! `type: item` AND `tags:` contains `pantry`. This keeps a
//! single physical thing visible in both lists — inventory's
//! scanner picks it up via `type: item`, ours filters down
//! to the food rows via the tag.

pub use vault_entity::ParseError;

use vault::VaultPage;
use vault_entity::store::VaultEntity;

use crate::entity::PantryItems;
use crate::model::PantryItem;

/// True when `page` is an inventory row tagged `pantry`.
#[must_use]
pub fn looks_like_pantry_item(page: &VaultPage) -> bool {
    PantryItems::matches(page)
}

pub fn parse_page(page: &VaultPage) -> Result<PantryItem, ParseError> {
    PantryItems::from_page(page)
}
