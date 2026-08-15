//! `vault_proto::VaultPage` → `Milestone`. Discriminator:
//! `type: milestone` in frontmatter (or `milestone` tag).
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `milestone::parse::*` paths working.

pub use vault_entity::ParseError;

use vault_entity::VaultEntity;
use vault_proto::VaultPage;

use crate::entity::Milestones;
use crate::model::Milestone;

/// True when `page` carries `type: milestone` (or the tag).
#[must_use]
pub fn looks_like_milestone(page: &VaultPage) -> bool {
    vault_entity::frontmatter::has_type(&page.raw, Milestones::TYPE)
}

pub fn parse_page(page: &VaultPage) -> Result<Milestone, ParseError> {
    parse_milestone(&page.rel_path, &page.basename, &page.raw)
}

pub fn parse_milestone(rel_path: &str, basename: &str, raw: &str) -> Result<Milestone, ParseError> {
    crate::entity::from_parts(rel_path, basename, raw)
}
