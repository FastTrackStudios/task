//! In-memory snapshot of an Obsidian vault.
//!
//! `Vault::open(root)` walks the tree and parses every `.md` and
//! `.base` file into a typed value. The result is a plain owned
//! struct — no shared signals, no CRDT, no async repo. Mutations
//! happen by editing a `VaultPage` (or its raw markdown) and
//! calling `Vault::save_page`.

use std::path::{Path, PathBuf};

use crate::obsidian_parse::{ParsedPage, parse_page, serialize_frontmatter};
use vault_live::bases::{self, ParsedBase};

use crate::config::{ObsidianConfig, read_obsidian_config};
use crate::guard::SelfWriteGuard;
use crate::walker::{VaultEntry, VaultEntryKind, walk_vault};

/// One `.md` file from the vault. Owned, parsed, with the original
/// raw bytes preserved so callers can diff against future writes.
#[derive(Clone, Debug)]
pub struct VaultPage {
    /// Vault-relative, forward-slash separated, e.g. `Music/Charts.md`.
    pub rel_path: String,
    /// Filename without extension.
    pub basename: String,
    /// Vault-relative parent directory; empty string at the root.
    pub folder: String,
    /// Raw file contents at scan time.
    pub raw: String,
    /// Parsed structure (frontmatter + flat block list).
    pub parsed: ParsedPage,
    /// `mtime` snapshot from disk. Used to detect external edits on
    /// reload.
    pub mtime: std::time::SystemTime,
}

/// One `.base` file.
#[derive(Clone, Debug)]
pub struct VaultBase {
    pub rel_path: String,
    pub basename: String,
    pub folder: String,
    pub raw: String,
    /// `Err` keeps the file visible in the sidebar even if its YAML
    /// is malformed. UI can show a warning chip.
    pub parsed: Result<ParsedBase, String>,
    pub mtime: std::time::SystemTime,
}

/// Untyped entry — currently only used so attachments / canvas
/// files show up in the sidebar.
#[derive(Clone, Debug)]
pub struct VaultEntryRef {
    pub rel_path: String,
    pub kind: VaultEntryKind,
}

/// In-memory snapshot of a vault root.
#[derive(Clone, Debug)]
pub struct Vault {
    pub root: PathBuf,
    pub config: ObsidianConfig,
    pub pages: Vec<VaultPage>,
    pub bases: Vec<VaultBase>,
    pub attachments: Vec<VaultEntryRef>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("io: {0}")]
    Io(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("io: {0}")]
    Io(String),
    #[error("page not found: {0}")]
    NotFound(String),
}

impl Vault {
    /// One-shot scan. Reads every `.md` / `.base` file; skips
    /// `.obsidian`, `.trash`, `.git`, dotfiles, and entries matched
    /// by `userIgnoreFilters` in `.obsidian/app.json`.
    pub fn open(root: &Path) -> Result<Self, LoadError> {
        let config = read_obsidian_config(root);
        let entries = walk_vault(root, &config);
        let mut pages = Vec::new();
        let mut bases = Vec::new();
        let mut attachments = Vec::new();

        for entry in entries {
            match entry.kind {
                VaultEntryKind::Markdown => match load_page(&entry) {
                    Ok(p) => pages.push(p),
                    Err(e) => tracing::warn!(path = %entry.rel_path, ?e, "page load failed"),
                },
                VaultEntryKind::Base => match load_base(&entry) {
                    Ok(b) => bases.push(b),
                    Err(e) => tracing::warn!(path = %entry.rel_path, ?e, "base load failed"),
                },
                VaultEntryKind::Canvas | VaultEntryKind::Attachment => {
                    attachments.push(VaultEntryRef {
                        rel_path: entry.rel_path,
                        kind: entry.kind,
                    });
                }
            }
        }

        Ok(Self {
            root: root.to_path_buf(),
            config,
            pages,
            bases,
            attachments,
        })
    }

    /// Look up a page by vault-relative path.
    #[must_use]
    pub fn page(&self, rel_path: &str) -> Option<&VaultPage> {
        self.pages.iter().find(|p| p.rel_path == rel_path)
    }

    pub fn page_mut(&mut self, rel_path: &str) -> Option<&mut VaultPage> {
        self.pages.iter_mut().find(|p| p.rel_path == rel_path)
    }

    /// Save a page's raw markdown back to disk, marking it on the
    /// self-write guard so the watcher can de-dupe the echo event.
    /// Also refreshes the page's `mtime` + cached `raw`.
    pub fn save_page(
        &mut self,
        rel_path: &str,
        new_raw: &str,
        guard: &SelfWriteGuard,
    ) -> Result<(), SaveError> {
        let abs = self.root.join(rel_path);
        let page = self
            .pages
            .iter_mut()
            .find(|p| p.rel_path == rel_path)
            .ok_or_else(|| SaveError::NotFound(rel_path.into()))?;

        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SaveError::Io(e.to_string()))?;
        }
        guard.mark(&abs);
        std::fs::write(&abs, new_raw).map_err(|e| SaveError::Io(e.to_string()))?;
        guard.mark(&abs);

        let mtime = std::fs::metadata(&abs)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now());
        page.raw = new_raw.to_string();
        page.parsed = parse_page(new_raw);
        page.mtime = mtime;
        Ok(())
    }

    /// Query: pages whose frontmatter has `key = value` (exact
    /// scalar match). Useful for Bases v0 and quick filters.
    #[must_use]
    pub fn pages_with_frontmatter(&self, key: &str, value: &serde_json::Value) -> Vec<&VaultPage> {
        self.pages
            .iter()
            .filter(|p| {
                p.parsed
                    .frontmatter
                    .iter()
                    .any(|e| e.key == key && &e.value == value)
            })
            .collect()
    }

    /// Query: pages tagged with `tag`. Reads both frontmatter `tags`
    /// (string, list-of-strings) and inline `#tag` markers parsed
    /// out by `parse_page` (which lands them in `ParsedBlock.refs`
    /// as `TagRef`).
    #[must_use]
    pub fn pages_with_tag(&self, tag: &str) -> Vec<&VaultPage> {
        self.pages.iter().filter(|p| page_has_tag(p, tag)).collect()
    }
}

fn page_has_tag(p: &VaultPage, tag: &str) -> bool {
    // Frontmatter `tags`.
    for e in &p.parsed.frontmatter {
        if e.key == "tags" || e.key == "tag" {
            match &e.value {
                serde_json::Value::String(s) => {
                    if s.split([',', ' '])
                        .any(|t| t.trim().trim_start_matches('#') == tag)
                    {
                        return true;
                    }
                }
                serde_json::Value::Array(arr) => {
                    if arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .any(|t| t.trim_start_matches('#') == tag)
                    {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    // Inline `#tag` refs parsed out of each block.
    for b in &p.parsed.blocks {
        for r in &b.refs {
            if let vault_live::refs::Ref::Tag(t) = r {
                if t.path.join("/") == tag {
                    return true;
                }
            }
        }
    }
    false
}

fn load_page(entry: &VaultEntry) -> Result<VaultPage, LoadError> {
    let raw = std::fs::read_to_string(&entry.abs_path).map_err(|e| LoadError::Io(e.to_string()))?;
    let parsed = parse_page(&raw);
    let mtime = std::fs::metadata(&entry.abs_path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| std::time::SystemTime::now());
    let (folder, basename) = split_rel(&entry.rel_path);
    Ok(VaultPage {
        rel_path: entry.rel_path.clone(),
        basename,
        folder,
        raw,
        parsed,
        mtime,
    })
}

fn load_base(entry: &VaultEntry) -> Result<VaultBase, LoadError> {
    let raw = std::fs::read_to_string(&entry.abs_path).map_err(|e| LoadError::Io(e.to_string()))?;
    let parsed = bases::parse(&raw).map_err(|e| e.to_string());
    let mtime = std::fs::metadata(&entry.abs_path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| std::time::SystemTime::now());
    let (folder, basename) = split_rel(&entry.rel_path);
    Ok(VaultBase {
        rel_path: entry.rel_path.clone(),
        basename,
        folder,
        raw,
        parsed,
        mtime,
    })
}

fn split_rel(rel: &str) -> (String, String) {
    let p = PathBuf::from(rel);
    let folder = p
        .parent()
        .map(|s| s.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let basename = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    (folder, basename)
}

/// Helper: rebuild a markdown file from a `ParsedPage`, preserving
/// frontmatter key order. Block-tree serialization is delegated to
/// `crate::obsidian_parse::serialize_page` once we have full
/// `Block` structs; for the FS-first phase, callers typically edit
/// raw markdown directly and pass it into `save_page`.
#[must_use]
pub fn serialize_parsed(parsed: &ParsedPage) -> String {
    // Frontmatter only; bodies are kept verbatim by callers.
    serialize_frontmatter(&parsed.frontmatter)
}

// Re-export so callers don't need a separate `knowledge_proto` dep
// for the common refs types used by query helpers.
pub use vault_live::refs;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn open_loads_pages_bases_and_attachments() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("Note.md"),
            "---\ntags: [chart]\n---\n\nBody\n",
        );
        touch(&dir.path().join("Folder/Sub.md"), "## H\n\n- bullet\n");
        touch(
            &dir.path().join("Charts.base"),
            "views:\n  - type: list\n    name: All\n",
        );
        touch(&dir.path().join("img.png"), "fake png");
        touch(&dir.path().join(".obsidian/app.json"), "{}");

        let v = Vault::open(dir.path()).expect("open");
        let rels: Vec<_> = v.pages.iter().map(|p| p.rel_path.as_str()).collect();
        assert!(rels.contains(&"Note.md"));
        assert!(rels.contains(&"Folder/Sub.md"));
        assert_eq!(v.bases.len(), 1);
        assert_eq!(v.bases[0].rel_path, "Charts.base");
        assert!(v.attachments.iter().any(|a| a.rel_path == "img.png"));
    }

    #[test]
    fn save_page_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("X.md"), "old\n");
        let mut v = Vault::open(dir.path()).unwrap();
        let guard = SelfWriteGuard::new();
        v.save_page("X.md", "new content\n", &guard).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("X.md")).unwrap();
        assert_eq!(on_disk, "new content\n");
        let page = v.page("X.md").unwrap();
        assert_eq!(page.raw, "new content\n");
    }

    #[test]
    fn frontmatter_query_finds_pages() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "---\nstatus: active\n---\n");
        touch(&dir.path().join("b.md"), "---\nstatus: archived\n---\n");
        let v = Vault::open(dir.path()).unwrap();
        let hits = v.pages_with_frontmatter("status", &serde_json::json!("active"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel_path, "a.md");
    }

    #[test]
    fn tag_query_finds_pages_from_frontmatter_and_inline() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("fm.md"), "---\ntags: [music]\n---\n");
        touch(&dir.path().join("inline.md"), "this is a #music note\n");
        touch(&dir.path().join("nope.md"), "no tags here\n");
        let v = Vault::open(dir.path()).unwrap();
        let hits: std::collections::HashSet<_> = v
            .pages_with_tag("music")
            .iter()
            .map(|p| p.rel_path.clone())
            .collect();
        assert!(hits.contains("fm.md"));
        assert!(hits.contains("inline.md"));
        assert!(!hits.contains("nope.md"));
    }
}
