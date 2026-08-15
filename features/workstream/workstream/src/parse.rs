//! `vault_proto::VaultPage` → `Workstream`. Discriminator:
//! `type: workstream` in frontmatter (or `workstream` tag).
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `workstream::parse::*` paths working.

pub use vault_entity::ParseError;

use vault_entity::VaultEntity;
use vault_proto::VaultPage;

use crate::entity::Workstreams;
use crate::model::Workstream;

/// True when `page` carries `type: workstream` (or the tag).
#[must_use]
pub fn looks_like_workstream(page: &VaultPage) -> bool {
    vault_entity::frontmatter::has_type(&page.raw, Workstreams::TYPE)
}

pub fn parse_page(page: &VaultPage) -> Result<Workstream, ParseError> {
    parse_workstream(&page.rel_path, &page.basename, &page.raw)
}

pub fn parse_workstream(
    rel_path: &str,
    basename: &str,
    raw: &str,
) -> Result<Workstream, ParseError> {
    crate::entity::from_parts(rel_path, basename, raw)
}
