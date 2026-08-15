//! Walk `<vault>/Wiki/`, build a list of pages with their
//! parsed frontmatter + body content. Skips the catalog
//! files (`schema.md`, `purpose.md`, `index.md`, `log.md`,
//! `overview.md`), the raw / state / media subtrees, and
//! any folder note named the same as its parent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use thiserror::Error;
use walkdir::WalkDir;
use wiki_proto::paths;

use crate::parse::{Page, parse_page};

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("wiki root not found at {0}")]
    NoWikiRoot(String),
}

const SKIP_FILES: &[&str] = &[
    paths::SCHEMA_MD,
    paths::PURPOSE_MD,
    paths::INDEX_MD,
    paths::LOG_MD,
    paths::OVERVIEW_MD,
];

const SKIP_PREFIXES: &[&str] = &["raw/", "_state/", "media/"];

/// Walk `<vault_root>/Wiki/` and return one `Page` per
/// markdown file we want indexed.
pub(crate) fn scan_wiki(vault_root: &Path) -> Result<Vec<Page>, ScanError> {
    let wiki_root = vault_root.join(paths::WIKI_ROOT);
    if !wiki_root.is_dir() {
        return Err(ScanError::NoWikiRoot(wiki_root.display().to_string()));
    }

    let mut by_path: HashMap<PathBuf, Page> = HashMap::new();
    for entry in WalkDir::new(&wiki_root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let rel = match path.strip_prefix(&wiki_root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().to_string();
        if SKIP_FILES.contains(&rel_str.as_str()) {
            continue;
        }
        if SKIP_PREFIXES
            .iter()
            .any(|p| rel_str.starts_with(p) || rel_str.starts_with(&format!("./{p}")))
        {
            continue;
        }
        // Skip folder notes (`Concepts/Concepts.md`) — they're
        // navigation shells, not content.
        if let Some(parent) = rel.parent() {
            if let (Some(folder_name), Some(stem)) = (
                parent.file_name().and_then(|s| s.to_str()),
                path.file_stem().and_then(|s| s.to_str()),
            ) {
                if folder_name == stem {
                    continue;
                }
            }
        }
        let body = std::fs::read_to_string(path)?;
        let page = parse_page(rel_str.clone(), &body);
        by_path.insert(rel, page);
    }

    let mut out: Vec<Page> = by_path.into_values().collect();
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Housekeeping dirs a prose-notes walk never descends into.
const NOTES_SKIP_DIRS: &[&str] = &[".obsidian", ".trash", ".git"];

/// Walk a prose-notes tree (typically the org vault) and return one
/// `Page` per markdown file, ids prefixed `note:/` so they can't
/// collide with wiki page paths when the two sets are merged (the
/// trailing slash keeps `Path::file_stem` — which drives Obsidian
/// link-by-stem resolution — working for top-level notes). Pages
/// without a `type:` frontmatter key default to `note` (not
/// `untyped`) so callers can filter on it. A missing root reads as
/// an empty tree, not an error — a fresh org has no notes yet.
pub(crate) fn scan_notes(notes_root: &Path) -> Result<Vec<Page>, ScanError> {
    let mut out: Vec<Page> = Vec::new();
    if !notes_root.is_dir() {
        return Ok(out);
    }
    for entry in WalkDir::new(notes_root)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| NOTES_SKIP_DIRS.contains(&n)))
        })
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(notes_root) else {
            continue;
        };
        let body = std::fs::read_to_string(path)?;
        let mut page = parse_page(format!("note:/{}", rel.to_string_lossy()), &body);
        if page.page_type == "untyped" {
            page.page_type = "note".to_string();
        }
        out.push(page);
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}
