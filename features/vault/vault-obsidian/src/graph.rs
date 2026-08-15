//! `VaultGraph` backend — answers the link-graph RPC
//! ([`vault_proto::VaultGraph`]) over [`LinkIndex`].
//!
//! Mirrors `vault_live::sync::Backend`'s shape: one
//! [`GraphBackend`] serves one-or-more vault roots addressed by
//! an opaque `vault_id`, mounted next to the sync backend on the
//! same per-org router. Read-only — every call re-opens the vault
//! snapshot from disk and rebuilds the index, the same freshness
//! contract `folder_index` already has. If profiling ever shows
//! the rebuild hot, cache `(mtime-watermark → LinkIndex)` here
//! without touching the wire trait.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use vault_proto::{GraphLink, GraphUnresolved, TagCount, VaultGraph, VaultSyncError};

use crate::links::LinkIndex;
use crate::vault::{Vault, VaultPage};
use vault_live::refs::Ref;

/// How the backend resolves `vault_id` → on-disk path. Same two
/// layouts as the sync backend.
#[derive(Debug, Clone)]
enum Layout {
    /// Explicit `vault_id → path` registry; unknown ids fail.
    Explicit(HashMap<String, PathBuf>),
    /// `{parent}/{vault_id}/` for any id; missing dirs read as
    /// empty vaults.
    UnderParent(PathBuf),
}

/// Filesystem-backed [`VaultGraph`] implementation. Cheap to
/// `Clone`. Construct it over the **same roots** as the
/// `vault_live::sync::Backend` serving the org so both services
/// describe the same files.
#[derive(Clone, architect::HasDispatcher)]
pub struct GraphBackend {
    layout: Layout,
}

impl GraphBackend {
    /// Serve a single vault under `root` as `vault_id`. Does not
    /// create the directory — this backend never writes; a
    /// missing root reads as an empty vault.
    #[must_use]
    pub fn single(vault_id: impl Into<String>, root: PathBuf) -> Self {
        let mut roots = HashMap::with_capacity(1);
        roots.insert(vault_id.into(), root);
        Self::with_roots(roots)
    }

    /// Serve a pre-built `vault_id → root` map.
    #[must_use]
    pub fn with_roots(roots: HashMap<String, PathBuf>) -> Self {
        Self {
            layout: Layout::Explicit(roots),
        }
    }

    /// Multi-tenant layout: every `vault_id` resolves to
    /// `{parent}/{vault_id}/`.
    #[must_use]
    pub fn under_parent(parent: PathBuf) -> Self {
        Self {
            layout: Layout::UnderParent(parent),
        }
    }

    fn root(&self, vault_id: &str) -> Result<PathBuf, VaultSyncError> {
        match &self.layout {
            Layout::Explicit(map) => map.get(vault_id).cloned().ok_or(VaultSyncError::NotFound),
            Layout::UnderParent(parent) => {
                // Refuse path-traversal-shaped ids, same stance as
                // the sync backend's file_path guard.
                if vault_id.is_empty()
                    || vault_id.contains('/')
                    || vault_id.contains('\\')
                    || vault_id.contains("..")
                {
                    return Err(VaultSyncError::BadPath);
                }
                Ok(parent.join(vault_id))
            }
        }
    }

    /// Open the vault snapshot, or `None` when the root doesn't
    /// exist yet (an empty vault, not an error).
    fn open(&self, vault_id: &str) -> Result<Option<Vault>, VaultSyncError> {
        let dir = self.root(vault_id)?;
        if !dir.exists() {
            return Ok(None);
        }
        Vault::open(Path::new(&dir))
            .map(Some)
            .map_err(|e| VaultSyncError::Internal(e.to_string()))
    }
}

impl VaultGraph for GraphBackend {
    fn backlinks(&self, vault_id: &str, path: &str) -> Result<Vec<String>, VaultSyncError> {
        let Some(vault) = self.open(vault_id)? else {
            return Ok(Vec::new());
        };
        let idx = LinkIndex::build(&vault);
        Ok(idx.backlinks(path).into_iter().map(str::to_owned).collect())
    }

    fn links(&self, vault_id: &str, path: &str) -> Result<Vec<GraphLink>, VaultSyncError> {
        let Some(vault) = self.open(vault_id)? else {
            return Ok(Vec::new());
        };
        let idx = LinkIndex::build(&vault);
        Ok(idx
            .outgoing(path)
            .into_iter()
            .map(|l| GraphLink {
                linkpath: l.linkpath.to_owned(),
                resolved: l.resolved.map(str::to_owned),
                alias: l.alias.map(str::to_owned),
            })
            .collect())
    }

    fn orphans(&self, vault_id: &str) -> Result<Vec<String>, VaultSyncError> {
        let Some(vault) = self.open(vault_id)? else {
            return Ok(Vec::new());
        };
        let idx = LinkIndex::build(&vault);
        Ok(idx.orphans().into_iter().map(str::to_owned).collect())
    }

    fn unresolved(&self, vault_id: &str) -> Result<Vec<GraphUnresolved>, VaultSyncError> {
        let Some(vault) = self.open(vault_id)? else {
            return Ok(Vec::new());
        };
        let idx = LinkIndex::build(&vault);
        Ok(idx
            .unresolved()
            .into_iter()
            .map(|u| GraphUnresolved {
                source: u.source.to_owned(),
                linkpath: u.linkpath.to_owned(),
            })
            .collect())
    }

    fn deadends(&self, vault_id: &str) -> Result<Vec<String>, VaultSyncError> {
        let Some(vault) = self.open(vault_id)? else {
            return Ok(Vec::new());
        };
        let idx = LinkIndex::build(&vault);
        Ok(idx.deadends().into_iter().map(str::to_owned).collect())
    }

    fn tags(&self, vault_id: &str) -> Result<Vec<TagCount>, VaultSyncError> {
        let Some(vault) = self.open(vault_id)? else {
            return Ok(Vec::new());
        };
        let mut counts: HashMap<String, u64> = HashMap::new();
        for page in &vault.pages {
            for tag in page_tags(page) {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }
        let mut rows: Vec<TagCount> = counts
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect();
        rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
        Ok(rows)
    }
}

/// Every tag on one page, de-duped: frontmatter `tags:` / `tag:`
/// (string or list form) plus inline `#tag` refs from the parsed
/// body. Normalized without the leading `#`; nested tags keep
/// their `parent/child` path.
fn page_tags(page: &VaultPage) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // Frontmatter forms — same parsing the CLI's `vault tags`
    // verb uses.
    for e in &page.parsed.frontmatter {
        if e.key != "tags" && e.key != "tag" {
            continue;
        }
        match &e.value {
            serde_json::Value::String(s) => {
                for t in s.split([',', ' ']) {
                    let t = t.trim().trim_start_matches('#');
                    if !t.is_empty() {
                        out.insert(t.to_string());
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        let t = s.trim().trim_start_matches('#');
                        if !t.is_empty() {
                            out.insert(t.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // Inline `#tag` markers parsed into `ParsedBlock.refs`.
    for block in &page.parsed.blocks {
        for r in &block.refs {
            if let Ref::Tag(t) = r {
                let tag = t.path.join("/");
                if !tag.is_empty() {
                    out.insert(tag);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, GraphBackend) {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("Wisdom.md"),
            "---\ntitle: Wisdom\ntags: [philosophy]\n---\nSee [[Plans]] and [[Nowhere]].\n",
        );
        touch(
            &dir.path().join("Plans.md"),
            "---\ntags: [philosophy, planning]\n---\nBack to [[Wisdom]]. Also #inline/tag here.\n",
        );
        touch(&dir.path().join("Loose.md"), "No links at all.\n");
        let backend = GraphBackend::single("v1", dir.path().to_path_buf());
        (dir, backend)
    }

    #[test]
    fn backlinks_links_orphans_deadends_unresolved() {
        let (_dir, b) = fixture();

        assert_eq!(b.backlinks("v1", "Wisdom.md").unwrap(), vec!["Plans.md"]);
        assert_eq!(b.backlinks("v1", "Plans.md").unwrap(), vec!["Wisdom.md"]);
        assert!(b.backlinks("v1", "Missing.md").unwrap().is_empty());

        let links = b.links("v1", "Wisdom.md").unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].resolved.as_deref(), Some("Plans.md"));
        assert_eq!(links[1].resolved, None, "Nowhere is unresolved");

        assert_eq!(b.orphans("v1").unwrap(), vec!["Loose.md"]);
        assert_eq!(b.deadends("v1").unwrap(), vec!["Loose.md"]);

        let unresolved = b.unresolved("v1").unwrap();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].source, "Wisdom.md");
        assert_eq!(unresolved[0].linkpath, "Nowhere");
    }

    #[test]
    fn tags_count_pages_once_and_include_inline() {
        let (_dir, b) = fixture();
        let tags = b.tags("v1").unwrap();
        let get = |name: &str| tags.iter().find(|t| t.tag == name).map(|t| t.count);
        assert_eq!(get("philosophy"), Some(2));
        assert_eq!(get("planning"), Some(1));
        assert_eq!(get("inline/tag"), Some(1));
        // Sorted by count desc then tag asc.
        assert_eq!(tags[0].tag, "philosophy");
    }

    #[test]
    fn unknown_vault_id_and_missing_root() {
        let (_dir, b) = fixture();
        assert!(matches!(
            b.backlinks("nope", "x.md"),
            Err(VaultSyncError::NotFound)
        ));

        let under = GraphBackend::under_parent(PathBuf::from("/nonexistent-parent-xyz"));
        assert!(under.backlinks("v1", "x.md").unwrap().is_empty());
        assert!(matches!(
            under.backlinks("../evil", "x.md"),
            Err(VaultSyncError::BadPath)
        ));
    }
}
