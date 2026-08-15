//! `vault::VaultPage` → `Location`.
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `locations::parse::*` paths working.

pub use vault_entity::ParseError;

use vault::VaultPage;
use vault_entity::store::VaultEntity;

use crate::entity::Locations;
use crate::model::Location;

/// True when `page` carries `type: location` (or the tag).
#[must_use]
pub fn looks_like_location(page: &VaultPage) -> bool {
    Locations::matches(page)
}

/// Parse a location page.
pub fn parse_page(page: &VaultPage) -> Result<Location, ParseError> {
    Locations::from_page(page)
}
