//! `VaultGraph` backend — answers the link-graph RPC
//! ([`vault_proto::VaultGraph`]) over [`LinkIndex`].
//!
//! Mirrors `vault_live::sync::Backend`'s shape: one
//! [`GraphBackend`] serves one-or-more vault roots addressed by
//! an opaque `vault_id`, mounted next to the sync backend on the
//! same per-org router. Read-only.
//!
//! Every answer comes from a [`GraphSnapshot`] — the whole graph,
//! owned — cached per vault and rebuilt only when the tree's
//! watermark (file count, newest mtime, total size) moves. It used
//! to re-open the vault and rebuild the index on EVERY call: a
//! flamegraph of production showed the server spending ~4.8 cores,
//! around the clock, parsing the same thousands of pages' YAML for
//! clients asking `links`/`backlinks`/`tags` every few seconds. The
//! watermark walk is a stat per file, every call — no trust window,
//! because a person who just saved a page must see its links on the
//! very next query; the parse is what cost, and that now happens once
//! per change.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use vault_proto::{GraphLink, GraphUnresolved, TagCount, VaultGraph, VaultSyncError};

use crate::links::LinkIndex;
use crate::vault::{Vault, VaultPage};
use vault_live::refs::Ref;

/// How the backend resolves `vault_id` → on-disk path. Same two
/// layouts as the sync backend.
#[derive(Debug, Clone)]
enum Layout {
    /// Explicit `vault_id → path` registry; unknown ids fail.
    /// Shared and growable ([`GraphBackend::add_root`]) so a wiki
    /// created at runtime gets a graph without a restart.
    Explicit(Arc<std::sync::RwLock<HashMap<String, PathBuf>>>),
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
    /// `vault_id → (watermark, snapshot)`. Shared by every clone —
    /// the dispatcher clones the backend per call, and a cache per
    /// clone would be no cache.
    cache: Arc<Mutex<HashMap<String, Cached>>>,
}

/// The whole link graph of one vault, owned, as the RPCs answer it.
///
/// [`LinkIndex`] borrows the [`Vault`] it indexes, which is right for
/// a one-shot query and wrong for a cache; this is the same
/// information with the borrows resolved, computed once per
/// watermark.
#[derive(Debug, Default)]
pub struct GraphSnapshot {
    backlinks: HashMap<String, Vec<String>>,
    outgoing: HashMap<String, Vec<GraphLink>>,
    orphans: Vec<String>,
    unresolved: Vec<GraphUnresolved>,
    deadends: Vec<String>,
    tags: Vec<TagCount>,
}

impl GraphSnapshot {
    #[must_use]
    pub fn build(vault: &Vault) -> Self {
        let idx = LinkIndex::build(vault);
        let mut backlinks = HashMap::new();
        let mut outgoing = HashMap::new();
        for page in &vault.pages {
            let path = page.rel_path.as_str();
            let incoming: Vec<String> =
                idx.backlinks(path).into_iter().map(str::to_owned).collect();
            if !incoming.is_empty() {
                backlinks.insert(path.to_owned(), incoming);
            }
            let out: Vec<GraphLink> = idx
                .outgoing(path)
                .into_iter()
                .map(|l| GraphLink {
                    linkpath: l.linkpath.to_owned(),
                    resolved: l.resolved.map(str::to_owned),
                    alias: l.alias.map(str::to_owned),
                })
                .collect();
            if !out.is_empty() {
                outgoing.insert(path.to_owned(), out);
            }
        }
        let mut counts: HashMap<String, u64> = HashMap::new();
        for page in &vault.pages {
            for tag in page_tags(page) {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }
        let mut tags: Vec<TagCount> = counts
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect();
        tags.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
        Self {
            backlinks,
            outgoing,
            orphans: idx.orphans().into_iter().map(str::to_owned).collect(),
            unresolved: idx
                .unresolved()
                .into_iter()
                .map(|u| GraphUnresolved {
                    source: u.source.to_owned(),
                    linkpath: u.linkpath.to_owned(),
                })
                .collect(),
            deadends: idx.deadends().into_iter().map(str::to_owned).collect(),
            tags,
        }
    }
}

/// What a vault tree looked like when its snapshot was built. Any
/// write moves at least one of these (a touch moves the mtime, a
/// create or delete the count, an in-place edit the size or mtime).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Watermark {
    files: usize,
    newest: Option<SystemTime>,
    bytes: u64,
}

struct Cached {
    watermark: Watermark,
    snapshot: Arc<GraphSnapshot>,
}

fn watermark(root: &Path) -> Watermark {
    let config = crate::config::read_obsidian_config(root);
    let mut w = Watermark {
        files: 0,
        newest: None,
        bytes: 0,
    };
    for entry in crate::walker::walk_vault(root, &config) {
        let Ok(meta) = std::fs::metadata(&entry.abs_path) else {
            continue;
        };
        w.files += 1;
        w.bytes = w.bytes.saturating_add(meta.len());
        if let Ok(m) = meta.modified() {
            w.newest = Some(w.newest.map_or(m, |n| n.max(m)));
        }
    }
    w
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
            layout: Layout::Explicit(Arc::new(std::sync::RwLock::new(roots))),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register one more `vault_id → root` (explicit layout only —
    /// `UnderParent` resolves any id already). Every clone sees it:
    /// the sync backend and this graph are handed the same roots, and
    /// a root added to one must be queryable through the other.
    pub fn add_root(&self, vault_id: impl Into<String>, root: PathBuf) {
        if let Layout::Explicit(map) = &self.layout {
            map.write()
                .expect("vault::graph roots poisoned")
                .insert(vault_id.into(), root);
        }
    }

    /// Multi-tenant layout: every `vault_id` resolves to
    /// `{parent}/{vault_id}/`.
    #[must_use]
    pub fn under_parent(parent: PathBuf) -> Self {
        Self {
            layout: Layout::UnderParent(parent),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn root(&self, vault_id: &str) -> Result<PathBuf, VaultSyncError> {
        match &self.layout {
            Layout::Explicit(map) => map
                .read()
                .expect("vault::graph roots poisoned")
                .get(vault_id)
                .cloned()
                .ok_or(VaultSyncError::NotFound),
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

    /// The vault's graph, from the cache when the tree has not moved
    /// since it was built. `None` when the root doesn't exist yet (an
    /// empty vault, not an error).
    fn snapshot(&self, vault_id: &str) -> Result<Option<Arc<GraphSnapshot>>, VaultSyncError> {
        let dir = self.root(vault_id)?;
        if !dir.exists() {
            return Ok(None);
        }
        let mark = watermark(&dir);
        if let Ok(c) = self.cache.lock()
            && let Some(entry) = c.get(vault_id)
            && entry.watermark == mark
        {
            return Ok(Some(Arc::clone(&entry.snapshot)));
        }
        // Parse outside the lock: a rebuild is the expensive part, and
        // a concurrent caller for another vault must not wait on it.
        let vault =
            Vault::open(Path::new(&dir)).map_err(|e| VaultSyncError::Internal(e.to_string()))?;
        let snapshot = Arc::new(GraphSnapshot::build(&vault));
        if let Ok(mut c) = self.cache.lock() {
            c.insert(
                vault_id.to_owned(),
                Cached {
                    watermark: mark,
                    snapshot: Arc::clone(&snapshot),
                },
            );
        }
        Ok(Some(snapshot))
    }

    /// How many vaults hold a cached snapshot right now. For tests.
    #[doc(hidden)]
    #[must_use]
    pub fn cached_vaults(&self) -> usize {
        self.cache.lock().map(|c| c.len()).unwrap_or(0)
    }
}

impl VaultGraph for GraphBackend {
    fn backlinks(&self, vault_id: &str, path: &str) -> Result<Vec<String>, VaultSyncError> {
        Ok(self
            .snapshot(vault_id)?
            .and_then(|g| g.backlinks.get(path).cloned())
            .unwrap_or_default())
    }

    fn links(&self, vault_id: &str, path: &str) -> Result<Vec<GraphLink>, VaultSyncError> {
        Ok(self
            .snapshot(vault_id)?
            .and_then(|g| g.outgoing.get(path).cloned())
            .unwrap_or_default())
    }

    fn orphans(&self, vault_id: &str) -> Result<Vec<String>, VaultSyncError> {
        Ok(self
            .snapshot(vault_id)?
            .map(|g| g.orphans.clone())
            .unwrap_or_default())
    }

    fn unresolved(&self, vault_id: &str) -> Result<Vec<GraphUnresolved>, VaultSyncError> {
        Ok(self
            .snapshot(vault_id)?
            .map(|g| g.unresolved.clone())
            .unwrap_or_default())
    }

    fn deadends(&self, vault_id: &str) -> Result<Vec<String>, VaultSyncError> {
        Ok(self
            .snapshot(vault_id)?
            .map(|g| g.deadends.clone())
            .unwrap_or_default())
    }

    fn tags(&self, vault_id: &str) -> Result<Vec<TagCount>, VaultSyncError> {
        Ok(self
            .snapshot(vault_id)?
            .map(|g| g.tags.clone())
            .unwrap_or_default())
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

    /// The snapshot is reused while the tree stands still, and a write
    /// — here a new page linking to an existing one — is seen on the
    /// very next call. The production symptom this guards: thousands
    /// of pages re-parsed per RPC.
    #[test]
    fn a_snapshot_is_reused_until_the_tree_moves() {
        let (dir, b) = fixture();
        assert_eq!(b.cached_vaults(), 0);
        let first = b.backlinks("v1", "Loose.md").unwrap();
        assert!(first.is_empty());
        assert_eq!(b.cached_vaults(), 1);
        // Same answer from the cache; the clone shares it (the
        // dispatcher hands every call a clone).
        assert_eq!(b.clone().backlinks("v1", "Loose.md").unwrap(), first);

        touch(&dir.path().join("Newcomer.md"), "Points at [[Loose]].\n");
        assert_eq!(
            b.backlinks("v1", "Loose.md").unwrap(),
            vec!["Newcomer.md".to_owned()]
        );
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
