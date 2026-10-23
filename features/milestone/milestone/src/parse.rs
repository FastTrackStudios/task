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
    looks_like_milestone_raw(&page.raw)
}

/// [`looks_like_milestone`] on a page's raw text, for a scan that borrows
/// the cached vault snapshot instead of building a `VaultPage`.
#[must_use]
pub fn looks_like_milestone_raw(raw: &str) -> bool {
    vault_entity::frontmatter::has_type(raw, Milestones::TYPE)
}

pub fn parse_page(page: &VaultPage) -> Result<Milestone, ParseError> {
    parse_milestone(&page.rel_path, &page.basename, &page.raw)
}

pub fn parse_milestone(rel_path: &str, basename: &str, raw: &str) -> Result<Milestone, ParseError> {
    crate::entity::from_parts(rel_path, basename, raw)
}
