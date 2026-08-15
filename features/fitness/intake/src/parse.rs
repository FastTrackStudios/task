//! `vault::VaultPage` → `IntakeLog`. Discriminator
//! `type: intake-log`.
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `intake::parse::*` paths working.

pub use vault_entity::ParseError;

use vault::VaultPage;
use vault_entity::store::VaultEntity;

use crate::entity::IntakeLogs;
use crate::model::IntakeLog;

/// True when `page` carries `type: intake-log` (or the tag).
pub fn looks_like_intake(page: &VaultPage) -> bool {
    IntakeLogs::matches(page)
}

/// Parse an intake-log page.
pub fn parse_page(page: &VaultPage) -> Result<IntakeLog, ParseError> {
    IntakeLogs::from_page(page)
}
