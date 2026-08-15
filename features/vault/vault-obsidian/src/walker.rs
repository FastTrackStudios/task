//! Walk a vault root in the shape Obsidian uses: arbitrary folder
//! tree under one root, with metadata in `.obsidian/`. Filters out
//! the conventional ignore set (`.obsidian`, `.trash`, `.git`,
//! `.DS_Store`, dotfiles) and honors `userIgnoreFilters` from
//! `.obsidian/app.json` (prefix / suffix patterns only in v1).

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::config::ObsidianConfig;

/// What an entry maps to in our entity model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultEntryKind {
    /// `.md` / `.markdown`. Parsed via `crate::obsidian_parse::parse_page`.
    Markdown,
    /// `.base` Obsidian Bases definition. Parsed via `vault_live::bases::parse`.
    Base,
    /// `.canvas`. v1 stores raw JSON on a placeholder page; full
    /// rendering is a later arc.
    Canvas,
    /// Anything else inside the vault — images, PDFs, audio, etc.
    /// Recorded so links can be resolved; no per-block import.
    Attachment,
}

#[derive(Clone, Debug)]
pub struct VaultEntry {
    /// Full disk path.
    pub abs_path: PathBuf,
    /// Vault-relative path with extension. Forward-slash separated.
    pub rel_path: String,
    pub kind: VaultEntryKind,
}

/// Synchronous walk. Returns entries in walkdir's default order
/// (folders before children, lexicographic within a folder). v1
/// loads the whole list into memory — vaults under ~50k files are
/// fine; we'll stream if/when that becomes a constraint.
pub fn walk_vault(root: &Path, cfg: &ObsidianConfig) -> Vec<VaultEntry> {
    let mut out = Vec::new();
    let ignore = build_ignore(cfg);
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_hard_skip(e.path(), root))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(?e, "walk error — skipping");
                continue;
            }
        };
        if entry.file_type().is_dir() {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let rel = match abs.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if ignore.matches(&rel) {
            continue;
        }
        let kind = classify(&abs);
        out.push(VaultEntry {
            abs_path: abs,
            rel_path: rel,
            kind,
        });
    }
    out
}

fn classify(p: &Path) -> VaultEntryKind {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => VaultEntryKind::Markdown,
        "base" => VaultEntryKind::Base,
        "canvas" => VaultEntryKind::Canvas,
        _ => VaultEntryKind::Attachment,
    }
}

/// Hard skips applied at the directory level so `walkdir` doesn't
/// descend into them at all. Top-level only for `.obsidian` /
/// `.trash` / `.git` — those are anchored at the vault root, not
/// nested. NOTE: we do NOT skip `node_modules` — Obsidian itself
/// indexes those trees (confirmed against The Observatory, where
/// its `files total` includes `node_modules` contents).
fn is_hard_skip(path: &Path, root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let components: Vec<_> = rel.components().collect();
    if components.is_empty() {
        return false;
    }
    let first = components[0].as_os_str().to_string_lossy();
    matches!(first.as_ref(), ".obsidian" | ".trash" | ".git") || first.starts_with(".DS_Store")
}

/// Compiled user-ignore matcher. v1 = prefix / suffix only.
struct IgnoreMatcher {
    prefixes: Vec<String>,
    suffixes: Vec<String>,
    exact: Vec<String>,
}

impl IgnoreMatcher {
    fn matches(&self, rel: &str) -> bool {
        if self.exact.iter().any(|e| e == rel) {
            return true;
        }
        if self.prefixes.iter().any(|p| rel.starts_with(p)) {
            return true;
        }
        if self.suffixes.iter().any(|s| rel.ends_with(s)) {
            return true;
        }
        false
    }
}

fn build_ignore(cfg: &ObsidianConfig) -> IgnoreMatcher {
    let mut prefixes = Vec::new();
    let mut suffixes = Vec::new();
    let mut exact = Vec::new();
    for raw in &cfg.user_ignore_filters {
        // Obsidian stores either bare paths or regex-y patterns; v1
        // only recognizes the simple shapes.
        if let Some(rest) = raw.strip_prefix('*').and_then(|r| r.strip_suffix('*')) {
            tracing::warn!(pattern = %raw, "ignoring unsupported user_ignore_filter (contains: {rest})");
        } else if let Some(rest) = raw.strip_prefix('*') {
            suffixes.push(rest.to_string());
        } else if let Some(rest) = raw.strip_suffix('*') {
            prefixes.push(rest.to_string());
        } else if raw.ends_with('/') {
            // Folder-style ignore: treat as prefix.
            prefixes.push(raw.clone());
        } else if !raw.contains('*') {
            exact.push(raw.clone());
        } else {
            tracing::warn!(pattern = %raw, "ignoring unsupported user_ignore_filter");
        }
    }
    IgnoreMatcher {
        prefixes,
        suffixes,
        exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, "x").unwrap();
    }

    #[test]
    fn walk_finds_markdown_and_bases_skips_obsidian_dir() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Note.md"));
        touch(&dir.path().join("Folder/Sub.md"));
        touch(&dir.path().join("Charts.base"));
        touch(&dir.path().join(".obsidian/app.json"));
        touch(&dir.path().join(".trash/old.md"));

        let cfg = ObsidianConfig::default();
        let entries = walk_vault(dir.path(), &cfg);
        let kinds: Vec<_> = entries
            .iter()
            .map(|e| (e.rel_path.clone(), e.kind))
            .collect();
        assert!(kinds.contains(&("Note.md".into(), VaultEntryKind::Markdown)));
        assert!(kinds.contains(&("Folder/Sub.md".into(), VaultEntryKind::Markdown)));
        assert!(kinds.contains(&("Charts.base".into(), VaultEntryKind::Base)));
        assert!(!kinds.iter().any(|(p, _)| p.starts_with(".obsidian")));
        assert!(!kinds.iter().any(|(p, _)| p.starts_with(".trash")));
    }

    #[test]
    fn node_modules_is_not_skipped() {
        // Obsidian indexes node_modules trees (confirmed against
        // The Observatory). We follow suit so file totals + the
        // backlink/orphan graph match.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("node_modules/foo/README.md"));
        let cfg = ObsidianConfig::default();
        let entries = walk_vault(dir.path(), &cfg);
        let rels: Vec<_> = entries.iter().map(|e| e.rel_path.clone()).collect();
        assert!(rels.contains(&"node_modules/foo/README.md".into()));
    }

    #[test]
    fn user_ignore_prefix_and_suffix() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Drafts/wip.md"));
        touch(&dir.path().join("Notes/keep.md"));
        touch(&dir.path().join("backup.tmp.md"));

        let cfg = ObsidianConfig {
            user_ignore_filters: vec!["Drafts/".into(), "*.tmp.md".into()],
            ..Default::default()
        };
        let entries = walk_vault(dir.path(), &cfg);
        let rels: Vec<_> = entries.iter().map(|e| e.rel_path.clone()).collect();
        assert!(rels.contains(&"Notes/keep.md".into()));
        assert!(!rels.iter().any(|r| r.starts_with("Drafts/")));
        assert!(!rels.iter().any(|r| r.ends_with(".tmp.md")));
    }
}
