//! `Wiki/index.md` — content catalog. Scans `Wiki/` for
//! `.md` pages, groups by `type:` frontmatter, renders a
//! markdown table.

use std::collections::BTreeMap;
use std::fs;

use walkdir::WalkDir;
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::vault::WikiLive;

const SKIP_FILES: &[&str] = &[
    paths::SCHEMA_MD,
    paths::PURPOSE_MD,
    paths::INDEX_MD,
    paths::LOG_MD,
    paths::OVERVIEW_MD,
];

impl WikiLive {
    /// Walk `Wiki/`, render `index.md` grouped by `type:`.
    /// Returns the resulting markdown body.
    pub fn rebuild_index(&self) -> Result<String, WikiLiveError> {
        if !self.is_bootstrapped() {
            return Err(WikiLiveError::NotBootstrapped);
        }
        let root = self.wiki_root();
        let mut grouped: BTreeMap<String, Vec<IndexEntry>> = BTreeMap::new();

        for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let rel = match path.strip_prefix(&root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let rel_str = rel.to_string_lossy().to_string();
            // Skip catalog files at the wiki root.
            if SKIP_FILES.contains(&rel_str.as_str()) {
                continue;
            }
            // Skip anything under raw/ or _state/ or media/.
            if rel_str.starts_with("raw/")
                || rel_str.starts_with("_state/")
                || rel_str.starts_with("media/")
            {
                continue;
            }
            let body = fs::read_to_string(path)?;
            let fm = parse_frontmatter(&body);
            let page_type = fm.get("type").cloned().unwrap_or_else(|| "untyped".into());
            let title = fm.get("title").cloned().unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string()
            });
            grouped.entry(page_type).or_default().push(IndexEntry {
                title,
                rel_path: rel_str,
            });
        }

        let mut out = String::from("# Wiki index\n\n");
        out.push_str(&format!(
            "Catalog of {} pages. LLM-maintained; rebuilt on every ingest pass.\n\n",
            grouped.values().map(std::vec::Vec::len).sum::<usize>()
        ));
        // Stable ordering: known types in convention order, then leftovers.
        let known = [
            "entity",
            "concept",
            "source",
            "synthesis",
            "comparison",
            "query",
        ];
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in &known {
            if let Some(entries) = grouped.get(*t) {
                seen.insert(*t);
                let mut entries = entries.clone();
                entries.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
                out.push_str(&format!("## {}\n\n", section_header_for_type(t)));
                for e in entries {
                    out.push_str(&format!("- [[{}]]\n", e.title));
                }
                out.push('\n');
            }
        }
        for (t, entries) in &grouped {
            if seen.contains(t.as_str()) {
                continue;
            }
            let mut entries = entries.clone();
            entries.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
            out.push_str(&format!("## {}\n\n", section_header_for_type(t)));
            for e in entries {
                out.push_str(&format!("- [[{}]]\n", e.title));
            }
            out.push('\n');
        }

        fs::write(root.join(paths::INDEX_MD), &out)?;
        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct IndexEntry {
    title: String,
    #[allow(dead_code)]
    rel_path: String,
}

fn section_header_for_type(t: &str) -> &str {
    match t {
        "entity" => "Entities",
        "concept" => "Concepts",
        "source" => "Sources",
        "synthesis" => "Synthesis",
        "comparison" => "Comparisons",
        "query" => "Queries",
        _ => "Untyped",
    }
}

/// Minimal frontmatter parser — only pulls flat
/// `key: value` lines from the leading `---`-fenced block.
fn parse_frontmatter(src: &str) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;
    let mut out = HashMap::new();
    let Some(rest) = src.strip_prefix("---\n") else {
        return out;
    };
    let Some(end) = rest.find("\n---\n") else {
        return out;
    };
    let fm = &rest[..end];
    for line in fm.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let mut v = v.trim().to_string();
            if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
                v = v[1..v.len() - 1].to_string();
            }
            if !k.is_empty() && !v.is_empty() {
                out.insert(k, v);
            }
        }
    }
    out
}

pub(crate) fn ensure_index(wiki: &WikiLive) -> Result<bool, WikiLiveError> {
    let path = wiki.wiki_root().join(paths::INDEX_MD);
    if path.is_file() {
        return Ok(false);
    }
    fs::write(&path, "# Wiki index\n\nNo pages yet.\n")?;
    Ok(true)
}
