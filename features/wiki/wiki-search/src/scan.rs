//! Shared walker — pulls every wiki page into memory once
//! per search call. Same skip rules as `wiki-graph::scan`
//! (no catalog files, no raw / state / media, no folder
//! notes).

use std::path::{Path, PathBuf};

use walkdir::WalkDir;
use wiki_proto::paths;

use crate::SearchError;

pub(crate) struct PageBody {
    pub(crate) rel_path: String,
    pub(crate) title: String,
    pub(crate) page_type: String,
    pub(crate) body: String,
}

const SKIP_FILES: &[&str] = &[
    paths::SCHEMA_MD,
    paths::PURPOSE_MD,
    paths::INDEX_MD,
    paths::LOG_MD,
    paths::OVERVIEW_MD,
];

pub(crate) fn scan(vault_root: &Path) -> Result<Vec<PageBody>, SearchError> {
    let wiki_root: PathBuf = vault_root.join(paths::WIKI_ROOT);
    if !wiki_root.is_dir() {
        return Err(SearchError::NoWikiRoot(wiki_root.display().to_string()));
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(&wiki_root).into_iter().filter_map(Result::ok) {
        let p = entry.path();
        if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let rel = match p.strip_prefix(&wiki_root) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => continue,
        };
        if SKIP_FILES.contains(&rel.as_str())
            || rel.starts_with("raw/")
            || rel.starts_with("_state/")
            || rel.starts_with("media/")
        {
            continue;
        }
        // Skip folder notes.
        if let (Some(parent), Some(stem)) = (
            std::path::Path::new(&rel)
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str()),
            std::path::Path::new(&rel)
                .file_stem()
                .and_then(|s| s.to_str()),
        ) {
            if parent == stem {
                continue;
            }
        }
        let raw = std::fs::read_to_string(p)?;
        let (title, page_type, body) = split_meta(&raw, &rel);
        out.push(PageBody {
            rel_path: rel,
            title,
            page_type,
            body,
        });
    }
    Ok(out)
}

fn split_meta(src: &str, rel: &str) -> (String, String, String) {
    let mut title = std::path::Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut page_type = "untyped".to_string();
    let body;
    if let Some(rest) = src.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let fm = &rest[..end];
            for line in fm.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let k = k.trim();
                    let mut v = v.trim().to_string();
                    if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
                        v = v[1..v.len() - 1].to_string();
                    }
                    if k == "title" {
                        title = v;
                    } else if k == "type" {
                        page_type = v;
                    }
                }
            }
            body = rest[end + 5..].to_string();
        } else {
            body = src.to_string();
        }
    } else {
        body = src.to_string();
    }
    (title, page_type, body)
}
