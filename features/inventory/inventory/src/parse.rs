//! `vault::VaultPage` → `Item`.
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `inventory::parse::*` paths working.

pub use vault_entity::ParseError;

use vault::VaultPage;
use vault_entity::store::VaultEntity;

use crate::entity::Items;
use crate::model::Item;

/// True when `page` carries `type: item` (or the tag).
#[must_use]
pub fn looks_like_item(page: &VaultPage) -> bool {
    Items::matches(page)
}

/// Parse an inventory-item page.
pub fn parse_page(page: &VaultPage) -> Result<Item, ParseError> {
    Items::from_page(page)
}
