//! `WikiBackend` — multi-vault wrapper around `WikiLive`
//! that implements the per-capability traits from
//! `wiki_proto::service`. Mounted on the task-server's
//! `/vox` endpoint via the architect-emitted descriptors.
//!
//! Single-vault deployments use [`Self::single`]; multi-
//! tenant servers use [`Self::with_roots`] or
//! [`Self::under_parent`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use wiki_proto::error::WikiError;
use wiki_proto::graph as gtypes;
use wiki_proto::health::WikiHealth as ProtoHealth;
use wiki_proto::ingest as itypes;
use wiki_proto::lint as ltypes;
use wiki_proto::log as ctypes;
use wiki_proto::pages as ptypes;
use wiki_proto::raw::{ImportRawSource, RawSourceRef};
use wiki_proto::review::ReviewItem;
use wiki_proto::schema as stypes;
use wiki_proto::search::{SearchHits, SearchOpts};
use wiki_proto::service::events::EventsStreamSource;
use wiki_proto::service::{
    Catalog, Graph, Ingest, Lint, Multimodal, Pages, RawLayer, Review, Schema, Search, Watcher,
};
use wiki_proto::{WikiChange, WikiEvent};

use crate::WikiLive;

#[derive(Clone)]
enum Layout {
    Explicit(HashMap<String, PathBuf>),
    UnderParent(PathBuf),
}

/// Cheaply clonable multi-vault backend.
#[derive(Clone, architect::HasDispatcher)]
pub struct WikiBackend {
    layout: Layout,
    /// Watcher state per wiki — `true` = watcher running.
    /// Implementation is stub-level (just records the
    /// toggle); spawning the actual watcher thread happens
    /// on `set_watch(true)`.
    watch_flags: Arc<std::sync::Mutex<HashMap<String, bool>>>,
    /// Fan-out hub behind the `Events` `#[subscribe]` stream. Every
    /// committed mutation publishes here (see [`Self::emit`]),
    /// wrapped with its `wiki_id` so subscribers — who see every
    /// wiki this backend serves — can filter. Sliding mailbox: a
    /// slow subscriber loses its oldest queued events and re-pulls
    /// when its stream re-establishes. Clones share the hub (`Arc`
    /// inside), so the service mount and the stream mount can each
    /// hold a backend clone.
    changes: architect::PubSub<WikiChange>,
}

impl WikiBackend {
    /// Single-vault server. `wiki_id` resolves to
    /// `vault_root`.
    pub fn single(wiki_id: impl Into<String>, vault_root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&vault_root)?;
        let mut roots = HashMap::with_capacity(1);
        roots.insert(wiki_id.into(), vault_root);
        Ok(Self::from_layout(Layout::Explicit(roots)))
    }

    /// Multi-tenant: `wiki_id` → `parent/{wiki_id}/`.
    pub fn under_parent(parent: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&parent)?;
        Ok(Self::from_layout(Layout::UnderParent(parent)))
    }

    #[must_use]
    pub fn with_roots(roots: HashMap<String, PathBuf>) -> Self {
        Self::from_layout(Layout::Explicit(roots))
    }

    fn from_layout(layout: Layout) -> Self {
        Self {
            layout,
            watch_flags: Arc::new(std::sync::Mutex::new(HashMap::new())),
            changes: architect::PubSub::sliding(256),
        }
    }

    /// Announce a committed change to every `changes` subscriber.
    /// Call only *after* the write landed — subscribers use these to
    /// decide what to re-fetch, so a speculative event costs a
    /// pointless round-trip (or worse, shows state that never was).
    fn emit(&self, wiki_id: &str, event: WikiEvent) {
        self.changes.publish(WikiChange {
            wiki_id: wiki_id.to_string(),
            event,
        });
    }

    fn resolve(&self, wiki_id: &str) -> Result<WikiLive, WikiError> {
        let root = match &self.layout {
            Layout::Explicit(map) => map
                .get(wiki_id)
                .cloned()
                .ok_or_else(|| WikiError::WikiNotFound(wiki_id.to_string()))?,
            Layout::UnderParent(parent) => parent.join(wiki_id),
        };
        Ok(WikiLive::open(root))
    }
}

// ────────────────────── Registry ──────────────────────

impl wiki_proto::service::registry::Registry for WikiBackend {
    /// t[impl wiki.many.addressable] — the org's whole set, so a
    /// client can pick a wiki instead of hard-coding one. This is the
    /// call that exists before a caller has a wiki id, which is why it
    /// takes none.
    fn list_wikis(&self) -> Result<Vec<wiki_proto::WikiSummary>, WikiError> {
        let mut out: Vec<wiki_proto::WikiSummary> = match &self.layout {
            Layout::Explicit(map) => map
                .iter()
                // The compatibility alias is a second name for a wiki
                // already in the list; listing it twice would show one
                // wiki as two.
                .filter(|(slug, _)| slug.as_str() != COMPAT_WIKI_ID)
                .map(|(slug, root)| summarize(slug, root))
                .collect(),
            Layout::UnderParent(parent) => std::fs::read_dir(parent)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let slug = e.file_name().to_str()?.to_owned();
                    Some(summarize(&slug, &e.path()))
                })
                .collect(),
        };
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(out)
    }
}

/// The slug the org's long-standing curated tier answers to.
const DEFAULT_SLUG: &str = "knowledge";

/// The id every client used before an org could hold more than one
/// wiki. Kept as an alias so an older caller still resolves; excluded
/// from listings so it never looks like a second wiki.
pub const COMPAT_WIKI_ID: &str = "default";

fn summarize(slug: &str, root: &std::path::Path) -> wiki_proto::WikiSummary {
    wiki_proto::WikiSummary {
        slug: slug.to_owned(),
        title: title_of(root).unwrap_or_else(|| prettify(slug)),
        pages: count_markdown(root),
        default: slug == DEFAULT_SLUG,
    }
}

/// A wiki's own name for itself, from `purpose.md`'s frontmatter.
fn title_of(root: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("purpose.md")).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("title:") {
            let t = rest.trim().trim_matches('"').trim();
            if !t.is_empty() {
                return Some(t.to_owned());
            }
        }
    }
    None
}

fn prettify(slug: &str) -> String {
    let mut out = String::with_capacity(slug.len());
    for (i, word) in slug.split('-').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn count_markdown(root: &std::path::Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut n = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // `_state/` is agent bookkeeping, not pages.
            if path.file_name().is_some_and(|f| f == "_state") {
                continue;
            }
            n += count_markdown(&path);
        } else if path.extension().is_some_and(|x| x == "md") {
            n += 1;
        }
    }
    n
}

// ────────────────────── Schema ──────────────────────
impl Schema for WikiBackend {
    fn bootstrap(&self, wiki_id: &str) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        w.bootstrap().map_err(map_err)?;
        Ok(())
    }
    fn read_schema(&self, wiki_id: &str) -> Result<stypes::SchemaDoc, WikiError> {
        let w = self.resolve(wiki_id)?;
        let path = w.wiki_root().join(wiki_proto::paths::SCHEMA_MD);
        let body = std::fs::read_to_string(&path).map_err(|e| WikiError::Io(e.to_string()))?;
        let modified = file_mtime(&path);
        Ok(stypes::SchemaDoc {
            markdown: body,
            modified,
        })
    }
    fn read_purpose(&self, wiki_id: &str) -> Result<stypes::PurposeDoc, WikiError> {
        let w = self.resolve(wiki_id)?;
        let path = w.wiki_root().join(wiki_proto::paths::PURPOSE_MD);
        let body = std::fs::read_to_string(&path).map_err(|e| WikiError::Io(e.to_string()))?;
        let modified = file_mtime(&path);
        Ok(stypes::PurposeDoc {
            markdown: body,
            modified,
        })
    }
    fn write_schema(&self, wiki_id: &str, markdown: &str) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        let path = w.wiki_root().join(wiki_proto::paths::SCHEMA_MD);
        std::fs::write(&path, markdown).map_err(|e| WikiError::Io(e.to_string()))
    }
    fn write_purpose(&self, wiki_id: &str, markdown: &str) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        let path = w.wiki_root().join(wiki_proto::paths::PURPOSE_MD);
        std::fs::write(&path, markdown).map_err(|e| WikiError::Io(e.to_string()))
    }
    fn health(&self, wiki_id: &str) -> Result<ProtoHealth, WikiError> {
        let w = self.resolve(wiki_id)?;
        let h = w.health().map_err(map_err)?;
        Ok(ProtoHealth {
            bootstrap_done: h.bootstrap_done,
            schema_present: h.schema_present,
            purpose_present: h.purpose_present,
            page_count: h.page_count,
            source_count: h.source_count,
            queue_depth: h.queue_depth,
            queue_failed: h.queue_failed,
            open_findings: 0,
            open_reviews: 0,
            research_in_flight: 0,
            peer_count: 0,
            last_lint_at: None,
            last_ingest_at: h.last_ingest_at,
            watching: self
                .watch_flags
                .lock()
                .ok()
                .and_then(|m| m.get(wiki_id).copied())
                .unwrap_or(false),
        })
    }
}

// ────────────────────── Catalog ──────────────────────
impl Catalog for WikiBackend {
    fn read_index(&self, wiki_id: &str) -> Result<ctypes::WikiIndex, WikiError> {
        let w = self.resolve(wiki_id)?;
        // Existing `rebuild_index` produces the markdown but
        // also returns it. We don't parse the markdown back —
        // a full structured index is a follow-up. For now,
        // return an empty parsed index (callers wanting the
        // markdown can read the file directly).
        let _ = w.rebuild_index().map_err(map_err)?;
        Ok(ctypes::WikiIndex {
            sections: Vec::new(),
            total: 0,
        })
    }
    fn rebuild_index(&self, wiki_id: &str) -> Result<ctypes::WikiIndex, WikiError> {
        let w = self.resolve(wiki_id)?;
        let _ = w.rebuild_index().map_err(map_err)?;
        Ok(ctypes::WikiIndex {
            sections: Vec::new(),
            total: 0,
        })
    }
    fn append_log(&self, wiki_id: &str, entry: ctypes::LogEntry) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        let op = match entry.op {
            ctypes::LogOp::Ingest => crate::log_md::LogOp::Ingest,
            ctypes::LogOp::Query => crate::log_md::LogOp::Query,
            ctypes::LogOp::Lint => crate::log_md::LogOp::Lint,
            ctypes::LogOp::Review => crate::log_md::LogOp::Review,
            ctypes::LogOp::Research => crate::log_md::LogOp::Research,
            ctypes::LogOp::Admin => crate::log_md::LogOp::Admin,
        };
        w.append_log(crate::log_md::LogEntry {
            at: entry.at,
            op,
            title: entry.title,
            body: entry.body,
            pages_touched: entry.pages_touched.0,
        })
        .map_err(map_err)
    }
}

// ────────────────────── RawLayer ──────────────────────
impl RawLayer for WikiBackend {
    fn import_raw_source(
        &self,
        wiki_id: &str,
        source: ImportRawSource,
    ) -> Result<RawSourceRef, WikiError> {
        let w = self.resolve(wiki_id)?;
        w.import_raw_source(source).map_err(map_err)
    }
    fn list_raw_sources(&self, wiki_id: &str) -> Result<Vec<RawSourceRef>, WikiError> {
        // No wiki-live helper yet — walk `raw/sources/` here.
        let w = self.resolve(wiki_id)?;
        let dir = w.wiki_root().join(wiki_proto::paths::SOURCES_DIR);
        let mut out = Vec::new();
        if !dir.is_dir() {
            return Ok(out);
        }
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(Result::ok)
        {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name = match p.file_name().and_then(|s| s.to_str()) {
                Some(n) if !n.starts_with('.') => n.to_string(),
                _ => continue,
            };
            let bytes = std::fs::read(p).map_err(|e| WikiError::Io(e.to_string()))?;
            let rel = p
                .strip_prefix(w.wiki_root())
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or(name.clone());
            out.push(RawSourceRef {
                path: rel,
                filename: name,
                mime: "application/octet-stream".to_string(),
                size: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
                imported_at: Utc::now(),
                title: String::new(),
            });
        }
        Ok(out)
    }
    fn read_raw_source(&self, wiki_id: &str, path: &str) -> Result<Vec<u8>, WikiError> {
        let w = self.resolve(wiki_id)?;
        w.read_raw_source(path).map_err(map_err)
    }
    fn delete_raw_source(&self, wiki_id: &str, path: &str) -> Result<Vec<ReviewItem>, WikiError> {
        let w = self.resolve(wiki_id)?;
        let abs = w.wiki_root().join(path);
        if abs.is_file() {
            std::fs::remove_file(&abs).map_err(|e| WikiError::Io(e.to_string()))?;
        }
        // Source-page orphan detection follows once the
        // review queue is wired through wiki-live.
        Ok(Vec::new())
    }
    fn rescan_sources(&self, wiki_id: &str) -> Result<Vec<itypes::IngestTask>, WikiError> {
        let w = self.resolve(wiki_id)?;
        let diff = w.rescan_sources().map_err(map_err)?;
        let mut tasks = Vec::new();
        for rel in diff.created.iter().chain(diff.modified.iter()) {
            let abs = w.wiki_root().join(rel);
            let Ok(bytes) = std::fs::read(&abs) else {
                continue;
            };
            let kind = if diff.created.contains(rel) {
                crate::queue::SourceChange::Created
            } else {
                crate::queue::SourceChange::Modified
            };
            let task = w.enqueue_ingest(rel, kind, &bytes).map_err(map_err)?;
            tasks.push(to_proto_task(task));
        }
        Ok(tasks)
    }
    fn rescan_diff(&self, wiki_id: &str) -> Result<wiki_proto::raw::SourceDiff, WikiError> {
        let w = self.resolve(wiki_id)?;
        let diff = w.rescan_sources().map_err(map_err)?;
        Ok(wiki_proto::raw::SourceDiff {
            created: diff.created,
            modified: diff.modified,
            deleted: diff.deleted,
        })
    }
}

// ────────────────────── Pages ──────────────────────
//
// The curated page layer — every `.md` under the wiki root
// except the `raw/`, `_state/` and `media/` subtrees. This is
// the surface the wiki UI's reader/editor drives; writes are
// sha-guarded so a stale editor can't clobber an agent's
// concurrent rewrite.
impl Pages for WikiBackend {
    fn list_pages(&self, wiki_id: &str) -> Result<Vec<ptypes::PageInfo>, WikiError> {
        let w = self.resolve(wiki_id)?;
        let root = w.wiki_root();
        let mut out = Vec::new();
        if !root.is_dir() {
            return Ok(out);
        }
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
        {
            let p = entry.path();
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Ok(rel) = p.strip_prefix(&root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if PAGE_SKIP_PREFIXES.iter().any(|pre| rel.starts_with(pre)) {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(p) else {
                continue;
            };
            let (title, page_type) = page_title_and_type(&rel, &body);
            let (ai_generated, generated_by) = page_provenance(&body);
            out.push(ptypes::PageInfo {
                path: rel,
                title,
                page_type,
                size: body.len() as u64,
                modified: mtime_utc(p),
                ai_generated,
                generated_by,
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn read_page(&self, wiki_id: &str, path: &str) -> Result<ptypes::WikiPageDoc, WikiError> {
        let w = self.resolve(wiki_id)?;
        let rel = sanitize_page_path(path)?;
        let abs = w.wiki_root().join(&rel);
        let markdown =
            std::fs::read_to_string(&abs).map_err(|_| WikiError::NotFound(rel.clone()))?;
        Ok(ptypes::WikiPageDoc {
            path: rel,
            sha256: sha256_hex(markdown.as_bytes()),
            modified: mtime_utc(&abs),
            markdown,
        })
    }

    fn write_page(
        &self,
        wiki_id: &str,
        path: &str,
        markdown: &str,
        base_sha256: &str,
    ) -> Result<ptypes::WikiPageDoc, WikiError> {
        let w = self.resolve(wiki_id)?;
        let rel = sanitize_page_path(path)?;
        let abs = w.wiki_root().join(&rel);
        if !base_sha256.is_empty() {
            if let Ok(current) = std::fs::read(&abs) {
                let current_sha = sha256_hex(&current);
                if current_sha != base_sha256 {
                    return Err(WikiError::IllegalState(format!(
                        "conflict: `{rel}` changed since it was read (expected {base_sha256}, found {current_sha})"
                    )));
                }
            }
        }
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WikiError::Io(format!("mkdir: {e}")))?;
        }
        std::fs::write(&abs, markdown)
            .map_err(|e| WikiError::Io(format!("write {}: {e}", abs.display())))?;
        let doc = ptypes::WikiPageDoc {
            path: rel,
            markdown: markdown.to_string(),
            sha256: sha256_hex(markdown.as_bytes()),
            modified: mtime_utc(&abs),
        };
        self.emit(
            wiki_id,
            WikiEvent::PageWritten {
                path: doc.path.clone(),
                at: doc.modified,
            },
        );
        Ok(doc)
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in
/// [`WikiBackend::emit`], on every committed mutation.
impl EventsStreamSource for WikiBackend {
    fn changes_hub(&self) -> &architect::PubSub<WikiChange> {
        &self.changes
    }
}

/// Subtrees that are not curated pages: the immutable raw
/// layer, opaque agent state, and extracted media.
const PAGE_SKIP_PREFIXES: &[&str] = &["raw/", "_state/", "media/", "."];

/// Normalize + validate a wiki-root-relative page path: no
/// absolute paths, no `..` escapes, `.md` only, never into the
/// raw / state / media subtrees.
fn sanitize_page_path(path: &str) -> Result<String, WikiError> {
    let rel = path.trim().trim_start_matches("./").replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') {
        return Err(WikiError::IllegalState(format!("bad page path: `{path}`")));
    }
    if rel.split('/').any(|seg| seg == ".." || seg.is_empty()) {
        return Err(WikiError::IllegalState(format!("bad page path: `{path}`")));
    }
    if !rel.ends_with(".md") {
        return Err(WikiError::IllegalState(format!(
            "not a markdown page: `{path}`"
        )));
    }
    if PAGE_SKIP_PREFIXES.iter().any(|pre| rel.starts_with(pre)) {
        return Err(WikiError::IllegalState(format!(
            "not a curated page (raw/state/media): `{path}`"
        )));
    }
    Ok(rel)
}

/// Frontmatter `title:` / `type:`, with the title falling back
/// to the first `# heading`, then the file stem.
/// Provenance frontmatter: `ai_generated: true` marks a page as
/// machine-produced (AI summary, ingest output) rather than the
/// user's own writing; `generated_by:` names the model/agent.
fn page_provenance(body: &str) -> (bool, String) {
    let mut ai_generated = false;
    let mut generated_by = String::new();
    if let Some(rest) = body.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let Some((k, v)) = line.split_once(':') else {
                    continue;
                };
                let v = v.trim().trim_matches('"').trim_matches('\'');
                match k.trim() {
                    "ai_generated" => ai_generated = v.eq_ignore_ascii_case("true"),
                    "generated_by" if generated_by.is_empty() => generated_by = v.to_string(),
                    _ => {}
                }
            }
        }
    }
    (ai_generated, generated_by)
}

fn page_title_and_type(rel_path: &str, body: &str) -> (String, String) {
    let mut title = String::new();
    let mut page_type = String::new();
    if let Some(rest) = body.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let Some((k, v)) = line.split_once(':') else {
                    continue;
                };
                let v = v.trim().trim_matches('"').trim_matches('\'');
                match k.trim() {
                    "title" if title.is_empty() => title = v.to_string(),
                    "type" if page_type.is_empty() => page_type = v.to_string(),
                    _ => {}
                }
            }
        }
    }
    if title.is_empty() {
        title = body
            .lines()
            .find_map(|l| l.strip_prefix("# "))
            .unwrap_or_default()
            .trim()
            .to_string();
    }
    if title.is_empty() {
        title = std::path::Path::new(rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(rel_path)
            .to_string();
    }
    (title, page_type)
}

/// Filesystem mtime as a `DateTime<Utc>`; epoch when
/// unavailable.
fn mtime_utc(path: &std::path::Path) -> chrono::DateTime<Utc> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(chrono::DateTime::<Utc>::from)
        .unwrap_or_else(|| chrono::DateTime::<Utc>::UNIX_EPOCH)
}

// ────────────────────── Graph ──────────────────────
impl Graph for WikiBackend {
    fn build_graph(
        &self,
        wiki_id: &str,
        opts: gtypes::GraphOpts,
    ) -> Result<gtypes::WikiGraph, WikiError> {
        let w = self.resolve(wiki_id)?;
        wiki_graph::build_graph(w.vault_root(), opts).map_err(|e| WikiError::Backend(e.to_string()))
    }
    fn relevance(
        &self,
        _wiki_id: &str,
        _from: &str,
        _to: &str,
    ) -> Result<gtypes::RelevanceScore, WikiError> {
        // Pairwise relevance lookup over the graph is a
        // small helper around `build_graph`; not yet on
        // wiki-graph's surface. Returning a zero score for
        // now so the trait is implementable; follow-up
        // wires it through.
        Ok(gtypes::RelevanceScore {
            direct_link: 0.0,
            source_overlap: 0.0,
            adamic_adar: 0.0,
            type_affinity: 0.0,
            total: 0.0,
        })
    }
    fn clusters(&self, wiki_id: &str) -> Result<Vec<gtypes::Cluster>, WikiError> {
        let w = self.resolve(wiki_id)?;
        wiki_graph::build_clusters(w.vault_root()).map_err(|e| WikiError::Backend(e.to_string()))
    }
    fn gaps(&self, wiki_id: &str) -> Result<Vec<gtypes::KnowledgeGap>, WikiError> {
        let w = self.resolve(wiki_id)?;
        wiki_graph::find_gaps(w.vault_root()).map_err(|e| WikiError::Backend(e.to_string()))
    }
}

// ────────────────────── Ingest ──────────────────────
impl Ingest for WikiBackend {
    fn enqueue_ingest(
        &self,
        wiki_id: &str,
        source_path: &str,
        change: itypes::SourceChange,
    ) -> Result<itypes::IngestTask, WikiError> {
        let w = self.resolve(wiki_id)?;
        let abs = w.wiki_root().join(source_path);
        let bytes = std::fs::read(&abs).map_err(|e| WikiError::Io(e.to_string()))?;
        let local_kind = match change {
            itypes::SourceChange::Created => crate::queue::SourceChange::Created,
            itypes::SourceChange::Modified => crate::queue::SourceChange::Modified,
            itypes::SourceChange::Deleted => crate::queue::SourceChange::Deleted,
        };
        let task = w
            .enqueue_ingest(source_path, local_kind, &bytes)
            .map_err(map_err)?;
        let task = to_proto_task(task);
        self.emit(
            wiki_id,
            WikiEvent::IngestEnqueued {
                task_id: task.id.clone(),
                source_path: source_path.to_string(),
            },
        );
        Ok(task)
    }
    fn list_ingest(&self, wiki_id: &str) -> Result<Vec<itypes::IngestTask>, WikiError> {
        let w = self.resolve(wiki_id)?;
        Ok(w.list_ingest()
            .map_err(map_err)?
            .into_iter()
            .map(to_proto_task)
            .collect())
    }
    fn claim_next_ingest(&self, wiki_id: &str) -> Result<Option<itypes::IngestTask>, WikiError> {
        let w = self.resolve(wiki_id)?;
        Ok(w.claim_next_ingest().map_err(map_err)?.map(to_proto_task))
    }
    fn record_analysis(
        &self,
        wiki_id: &str,
        task_id: &str,
        analysis: itypes::AnalysisDraft,
    ) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        // Squash the structured AnalysisDraft into the
        // free-form `notes` body that wiki-live persists.
        let body = serde_json::to_string_pretty(&serde_json::json!({
            "notes": analysis.notes,
            "entities": analysis.entities.iter().map(|e| serde_json::json!({"name": e.name, "summary": e.summary})).collect::<Vec<_>>(),
            "concepts": analysis.concepts.iter().map(|c| serde_json::json!({"name": c.name, "summary": c.summary})).collect::<Vec<_>>(),
        })).unwrap_or_default();
        w.record_analysis(task_id, body).map_err(map_err)?;
        Ok(())
    }
    fn record_pages(
        &self,
        wiki_id: &str,
        task_id: &str,
        pages: Vec<itypes::PageDraft>,
    ) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        let drafts: Vec<crate::queue::PageDraft> = pages
            .into_iter()
            .map(|p| crate::queue::PageDraft {
                path: p.path,
                markdown: p.markdown,
                overwrite: p.overwrite,
            })
            .collect();
        w.record_pages(task_id, &drafts).map_err(map_err)?;
        Ok(())
    }
    fn fail_ingest(&self, wiki_id: &str, task_id: &str, error: &str) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        w.fail_ingest(task_id, error).map_err(map_err)?;
        Ok(())
    }
    fn cancel_ingest(&self, _wiki_id: &str, _task_id: &str) -> Result<(), WikiError> {
        // Cancellation flips the queue row's status — needs
        // a small helper on wiki-live. Stub for now.
        Ok(())
    }
    fn retry_ingest(&self, wiki_id: &str, task_id: &str) -> Result<itypes::IngestTask, WikiError> {
        let w = self.resolve(wiki_id)?;
        // Look up the failed task and re-enqueue as a fresh
        // task. Same source path, sha256, increments retry
        // count via wiki-live's bookkeeping (follow-up).
        for task in w.list_ingest().map_err(map_err)? {
            if task.id == task_id {
                let abs = w.wiki_root().join(&task.source_path);
                if let Ok(bytes) = std::fs::read(&abs) {
                    let new_task = w
                        .enqueue_ingest(
                            &task.source_path,
                            crate::queue::SourceChange::Modified,
                            &bytes,
                        )
                        .map_err(map_err)?;
                    return Ok(to_proto_task(new_task));
                }
            }
        }
        Err(WikiError::UnknownTask(task_id.to_string()))
    }
}

// ────────────────────── Lint ──────────────────────
impl Lint for WikiBackend {
    fn lint(
        &self,
        _wiki_id: &str,
        _scope: ltypes::LintScope,
    ) -> Result<Vec<ltypes::LintFinding>, WikiError> {
        // LLM-driven lint runs through the agent-wiki
        // bridge. Trait surface returns empty so callers
        // get a clean result; the bridge persists findings
        // directly to `Wiki/_state/lint_findings.json`,
        // which `list_findings` surfaces.
        Ok(Vec::new())
    }
    fn list_findings(&self, wiki_id: &str) -> Result<Vec<ltypes::LintFinding>, WikiError> {
        let w = self.resolve(wiki_id)?;
        let local = w
            .list_findings(Some(crate::FindingStatus::Open))
            .map_err(map_err)?;
        Ok(local.into_iter().map(to_proto_finding).collect())
    }
    fn resolve_finding(
        &self,
        wiki_id: &str,
        finding_id: &str,
        action: ltypes::FindingAction,
    ) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        match action {
            ltypes::FindingAction::Resolve => {
                w.resolve_finding(finding_id).map_err(map_err)?;
            }
            ltypes::FindingAction::Dismiss { .. } => {
                w.resolve_finding(finding_id).map_err(map_err)?;
            }
            ltypes::FindingAction::PromoteToReview | ltypes::FindingAction::PromoteToResearch => {
                // Promote-to-{review,research} needs the
                // matching queues on wiki-live; stub flips
                // to resolved for now.
                w.resolve_finding(finding_id).map_err(map_err)?;
            }
        }
        Ok(())
    }
}

// ────────────────────── Search ──────────────────────
impl Search for WikiBackend {
    fn search(&self, wiki_id: &str, opts: SearchOpts) -> Result<SearchHits, WikiError> {
        let w = self.resolve(wiki_id)?;
        wiki_search::search(w.vault_root(), opts).map_err(|e| WikiError::Backend(e.to_string()))
    }
}

// ────────────────────── Watcher ──────────────────────
impl Watcher for WikiBackend {
    fn set_watch(&self, wiki_id: &str, enabled: bool) -> Result<bool, WikiError> {
        // Toggle the recorded state. Spawning the actual
        // FS watcher per-wiki happens on the local CLI
        // (`task wiki watch-sources`) — the server-side
        // version follows when the run loop lands.
        let mut flags = self
            .watch_flags
            .lock()
            .map_err(|e| WikiError::Backend(format!("watch_flags poisoned: {e}")))?;
        flags.insert(wiki_id.to_string(), enabled);
        Ok(enabled)
    }
    fn is_watching(&self, wiki_id: &str) -> Result<bool, WikiError> {
        Ok(self
            .watch_flags
            .lock()
            .ok()
            .and_then(|m| m.get(wiki_id).copied())
            .unwrap_or(false))
    }
}

// ────────────────────── helpers ──────────────────────

fn file_mtime(path: &std::path::Path) -> String {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339().into())
        .unwrap_or_default()
}

fn map_err(e: crate::WikiLiveError) -> WikiError {
    match e {
        crate::WikiLiveError::NotBootstrapped => {
            WikiError::SchemaMissing("not bootstrapped".to_string())
        }
        crate::WikiLiveError::Io(io) => WikiError::Io(io.to_string()),
        crate::WikiLiveError::Yaml(s) => WikiError::MalformedFrontmatter(String::new(), s),
        crate::WikiLiveError::Json(j) => WikiError::Backend(format!("json: {j}")),
        crate::WikiLiveError::IllegalState(s) => WikiError::IllegalState(s),
        crate::WikiLiveError::RawIsImmutable { path } => {
            WikiError::IllegalState(format!("raw is immutable: {path}"))
        }
        crate::WikiLiveError::PathEscape { path } => {
            WikiError::IllegalState(format!("path escape: {path}"))
        }
        crate::WikiLiveError::TaskNotFound(id) => WikiError::UnknownTask(id),
    }
}

fn to_proto_task(t: crate::queue::IngestTask) -> itypes::IngestTask {
    itypes::IngestTask {
        id: t.id,
        source_path: t.source_path,
        kind: match t.kind {
            crate::queue::SourceChange::Created => itypes::SourceChange::Created,
            crate::queue::SourceChange::Modified => itypes::SourceChange::Modified,
            crate::queue::SourceChange::Deleted => itypes::SourceChange::Deleted,
        },
        status: match t.status {
            crate::queue::IngestStatus::Pending => itypes::IngestStatus::Pending,
            crate::queue::IngestStatus::Analyzing => itypes::IngestStatus::Analyzing,
            crate::queue::IngestStatus::Generating => itypes::IngestStatus::Generating,
            crate::queue::IngestStatus::Writing => itypes::IngestStatus::Writing,
            crate::queue::IngestStatus::Done => itypes::IngestStatus::Done,
            crate::queue::IngestStatus::Failed => itypes::IngestStatus::Failed,
            crate::queue::IngestStatus::Cancelled => itypes::IngestStatus::Cancelled,
        },
        source_sha256: t.source_sha256,
        // Structured analysis isn't kept on disk — only the
        // serialized blob from `record_analysis`. Return
        // an empty draft over the wire; callers re-derive
        // from the queue body if needed.
        analysis: None,
        pages: Vec::new(),
        retries: t.retries,
        last_error: t.last_error,
        enqueued_at: t.enqueued_at,
        updated_at: t.updated_at,
    }
}

fn to_proto_finding(f: crate::LintFinding) -> ltypes::LintFinding {
    ltypes::LintFinding {
        id: f.id,
        scope: match f.kind {
            crate::LintKind::Contradiction => ltypes::LintScope::Contradiction,
            crate::LintKind::Stale => ltypes::LintScope::Stale,
            crate::LintKind::MissingPage => ltypes::LintScope::MissingPage,
            crate::LintKind::Suggestion => ltypes::LintScope::All,
        },
        subjects: f.pages,
        message: f.title,
        severity: match f.severity {
            crate::LintSeverity::Warning => ltypes::Severity::Warn,
            crate::LintSeverity::Info => ltypes::Severity::Info,
        },
        suggestion: f.description,
        raised_at: f.raised_at,
        status: match f.status {
            crate::FindingStatus::Open => ltypes::FindingStatus::Open,
            crate::FindingStatus::Resolved => ltypes::FindingStatus::Resolved,
            crate::FindingStatus::Dismissed => ltypes::FindingStatus::Dismissed,
        },
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

// ────────────────────── Review ──────────────────────
//
// Backed by `Wiki/_state/review.json` (see
// `crate::reviews`). The trait surface accepts a full
// [`ReviewItem`] on enqueue + an action label on apply; we
// translate to the local mirror via `WikiLive::enqueue_review`
// / `list_review` / `mark_review_resolved`.
impl Review for WikiBackend {
    fn enqueue_review(&self, wiki_id: &str, item: ReviewItem) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        let item_id = item.id.clone();
        w.enqueue_review(item).map_err(map_err)?;
        self.emit(wiki_id, WikiEvent::ReviewEnqueued { item_id });
        Ok(())
    }

    fn list_review(&self, wiki_id: &str) -> Result<Vec<ReviewItem>, WikiError> {
        let w = self.resolve(wiki_id)?;
        w.list_review().map_err(map_err)
    }

    fn apply_review(
        &self,
        wiki_id: &str,
        item_id: &str,
        action: wiki_proto::review::ReviewAction,
    ) -> Result<(), WikiError> {
        let w = self.resolve(wiki_id)?;
        // Side-effect each action kind, then mark the item
        // resolved or dismissed. Backends decide the on-disk
        // convention for `AppendNote` — here we append under a
        // trailing `## Edit log` section, creating it if missing.
        match &action {
            wiki_proto::review::ReviewAction::RewritePage { path, markdown } => {
                let abs = w.wiki_root().join(path);
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| WikiError::Io(format!("mkdir: {e}")))?;
                }
                std::fs::write(&abs, markdown)
                    .map_err(|e| WikiError::Io(format!("write {}: {e}", abs.display())))?;
                w.mark_review_resolved(item_id).map_err(map_err)?;
            }
            wiki_proto::review::ReviewAction::AppendNote { path, body } => {
                let abs = w.wiki_root().join(path);
                let existing = std::fs::read_to_string(&abs).unwrap_or_default();
                let new_body = if existing.contains("\n## Edit log\n") {
                    format!("{}\n{}\n", existing.trim_end(), body)
                } else {
                    format!("{}\n\n## Edit log\n\n{}\n", existing.trim_end(), body)
                };
                std::fs::write(&abs, new_body)
                    .map_err(|e| WikiError::Io(format!("append {}: {e}", abs.display())))?;
                w.mark_review_resolved(item_id).map_err(map_err)?;
            }
            wiki_proto::review::ReviewAction::Research { query: _ } => {
                // ResearchPlan promotion is the Research
                // service's job; for now we just mark the
                // review resolved + the curator can drive a
                // research plan separately via the CLI.
                w.mark_review_resolved(item_id).map_err(map_err)?;
            }
            wiki_proto::review::ReviewAction::AcceptNoOp => {
                w.mark_review_resolved(item_id).map_err(map_err)?;
            }
            wiki_proto::review::ReviewAction::Dismiss { reason: _ } => {
                w.mark_review_dismissed(item_id).map_err(map_err)?;
            }
        }
        Ok(())
    }
}

// ────────────────────── Multimodal ──────────────────────
impl Multimodal for WikiBackend {
    fn extract_images(
        &self,
        wiki_id: &str,
        source_path: &str,
        opts: wiki_proto::multimodal::ExtractOpts,
    ) -> Result<Vec<wiki_proto::multimodal::ExtractedImage>, WikiError> {
        let w = self.resolve(wiki_id)?;
        let abs = w.wiki_root().join(source_path);
        wiki_extract::extract_path(&abs, &opts)
            .map_err(|e| WikiError::Backend(format!("extract: {e}")))
    }
}
