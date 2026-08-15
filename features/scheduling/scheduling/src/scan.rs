//! Vault scanner — walks a `vault::Vault` and yields scheduling
//! entities by frontmatter `type:` discriminator.
//!
//! v1 stub for [`scan_day_templates`] kept around as a worked
//! example. The real entry points used by `VaultScheduler` live
//! directly on the scheduler (it walks `<root>/scheduling/`
//! subdirectories with the per-entity parsers in `parse::`).
//! Both paths share the [`frontmatter_split`] helper.

use thiserror::Error;

use scheduling_proto::DayTemplate;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("parse error in {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: crate::parse::ParseError,
    },
    #[error("vault io: {0}")]
    Vault(String),
}

/// Split a markdown file's leading YAML frontmatter from the
/// body. Returns `(frontmatter, body)`. Mirrors the parser in
/// `task::parse`.
#[must_use]
pub fn frontmatter_split(src: &str) -> Option<(&str, &str)> {
    let rest = src.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

/// Empty stub. Kept for the legacy import path — the real
/// scanner lives on `VaultScheduler::scan`.
pub fn scan_day_templates(_vault: &vault::Vault) -> Result<Vec<DayTemplate>, ScanError> {
    Ok(Vec::new())
}
