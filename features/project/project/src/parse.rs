//! `VaultPage` (or raw markdown) → [`ProjectInfo`].
//!
//! The field mapping lives in [`crate::entity`]; this module keeps the
//! historical `project::parse::*` paths working.

pub use vault_entity::ParseError;

use vault_proto::VaultPage;

use crate::model::ProjectInfo;

/// `true` if the page declares itself as a project. Two
/// shapes accepted:
///
/// - `type: project` in the frontmatter, or
/// - `tags: [..., project]` (case-insensitive on `project`).
#[must_use]
pub fn looks_like_project(page: &VaultPage) -> bool {
    crate::entity::matches_raw(&page.raw)
}

/// Parse a `VaultPage` into a `ProjectInfo`. The page must
/// carry frontmatter; missing optional fields default.
pub fn parse_page(page: &VaultPage) -> Result<ProjectInfo, ParseError> {
    crate::entity::from_parts(&page.rel_path, &page.basename, &page.raw)
}

/// Parse raw markdown into a `ProjectInfo`. `rel_path` and
/// `basename` only feed defaults (basename fills `title`
/// when frontmatter omits it; `rel_path` becomes
/// `ProjectInfo::path`).
pub fn parse_str(rel_path: &str, basename: &str, raw: &str) -> Result<ProjectInfo, ParseError> {
    crate::entity::from_parts(rel_path, basename, raw)
}
