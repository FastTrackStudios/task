//! `WikiBackend` — multi-vault wrapper around `WikiLive`
//! that implements the per-capability traits from
//! `wiki_proto::service`. Mounted on the task-server's
//! `/vox` endpoint via the architect-emitted descriptors.
//!
//! Single-vault deployments use [`Self::single`]; multi-
//! tenant servers use [`Self::with_roots`] or
//! [`Self::under_parent`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::Utc;
use wiki_proto::config::{NewWiki, Visibility, WikiConfig};
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
use wiki_proto::service::registry::{WikiDescription, WikiSummary};
use wiki_proto::service::{
    Catalog, Graph, Ingest, Lint, Multimodal, Pages, RawLayer, Review, Schema, Search, Watcher,
};
use wiki_proto::{WikiChange, WikiEvent};

use crate::WikiLive;

/// How wiki ids become directories.
///
/// `Explicit` is the shape the server mounts: every wiki the org holds
/// at boot, keyed by slug, plus the directory new ones are created in.
/// The map is behind a lock because the set changes while the server
/// runs (`wiki.many.set`) — `create_wiki` adds to it and `delete_wiki`
/// removes from it, and every clone of the backend sees the change.
///
/// `UnderParent` is the older multi-tenant shape, `parent/<id>/`, where
/// the filesystem is the map.
#[derive(Clone)]
enum Layout {
    Explicit {
        roots: Arc<RwLock<HashMap<String, PathBuf>>>,
        /// Where a created wiki goes: `<org>/wikis/`. `None` when the
        /// backend was built from a fixed map with nowhere to grow,
        /// which is what a single-root test wants.
        wikis_dir: Option<PathBuf>,
    },
    UnderParent(PathBuf),
}

/// Who is calling, as far as the wiki lane needs to know.
///
/// The gate in front of the org router has already decided the caller
/// may reach this org; what the lane still needs is *which account*,
/// so it can record a proposer and check Editor. The default reads
/// `architect`'s per-call principal; the server can swap in something
/// richer, and a test can pin a caller.
pub trait Caller: Send + Sync + 'static {
    /// The account id the gate resolved, or `None` for an in-process
    /// call — the server acting on its own behalf.
    fn principal(&self) -> Option<String>;
    /// Whether the caller holds the org's admin role. Admins may grant
    /// Editor and change what the set holds (`wiki.edit.editor`).
    fn is_org_admin(&self) -> bool {
        false
    }
    /// Whether the caller is a member of the owning org. On the org
    /// router every caller the gate admitted is one; the hook exists
    /// for the account lane and for peers relaying a request, whose
    /// proposals a `Members` gate holds rather than publishes
    /// (`wiki.edit.gate`).
    fn is_org_member(&self) -> bool {
        true
    }
}

/// Run work that may take a while — a graph build, a search, a git
/// fetch — without holding a runtime worker.
///
/// The backend is dispatched inline so the caller task-local survives
/// (see [`WikiBackend`]); the price is that a slow method would block
/// the async worker that called it. `block_in_place` hands the worker's
/// duties to another thread for the duration and keeps the task — and
/// its task-locals — where they are. Outside a multi-thread runtime
/// (a unit test, a current-thread runtime) it simply runs `f`.
pub(crate) fn blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// The default [`Caller`]: whatever the permissions gate recorded for
/// this call.
pub struct GateCaller;

impl Caller for GateCaller {
    fn principal(&self) -> Option<String> {
        match architect::permissions_gate::caller() {
            Some(architect_permissions::Principal::User { user_id }) => Some(user_id),
            _ => None,
        }
    }
}

/// Cheaply clonable multi-vault backend.
///
/// Dispatched inline: `create_wiki` names its creator and `write_page`
/// checks Editor from the caller the gate recorded in a task-local,
/// which a `spawn_blocking` hop would lose — every caller would read as
/// the server itself, and the Edit lane's check on direct writes
/// (`wiki.edit.editor`) would pass anyone.
#[derive(Clone, architect::HasDispatcher)]
#[dispatch(architect::dispatch::CurrentThreadDispatcher)]
pub struct WikiBackend {
    layout: Layout,
    caller: Arc<dyn Caller>,
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
    /// Told `(slug, root)` after `create_wiki` commits — the server's
    /// way of registering the new directory with the other services
    /// that serve wiki pages (vault-sync, the link graph, collab).
    on_created: Option<WikiCreatedHook>,
}

/// What [`WikiBackend::with_on_created`] is handed: a callback run
/// with the new wiki's slug and root directory once it exists on
/// disk and in the set.
pub type WikiCreatedHook = Arc<dyn Fn(&str, &Path) + Send + Sync>;

impl WikiBackend {
    /// Single-vault server. `wiki_id` resolves to
    /// `vault_root`.
    pub fn single(wiki_id: impl Into<String>, vault_root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&vault_root)?;
        let mut roots = HashMap::with_capacity(1);
        roots.insert(wiki_id.into(), vault_root);
        Ok(Self::from_layout(Layout::Explicit {
            roots: Arc::new(RwLock::new(roots)),
            wikis_dir: None,
        }))
    }

    /// Multi-tenant: `wiki_id` → `parent/{wiki_id}/`.
    pub fn under_parent(parent: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&parent)?;
        Ok(Self::from_layout(Layout::UnderParent(parent)))
    }

    /// A fixed set. Nothing can be created into it; see
    /// [`Self::with_roots_under`] for the shape the server mounts.
    #[must_use]
    pub fn with_roots(roots: HashMap<String, PathBuf>) -> Self {
        Self::from_layout(Layout::Explicit {
            roots: Arc::new(RwLock::new(roots)),
            wikis_dir: None,
        })
    }

    /// The set an org holds at boot, plus the directory new wikis are
    /// created in (`<org>/wikis/`).
    #[must_use]
    pub fn with_roots_under(roots: HashMap<String, PathBuf>, wikis_dir: PathBuf) -> Self {
        Self::from_layout(Layout::Explicit {
            roots: Arc::new(RwLock::new(roots)),
            wikis_dir: Some(wikis_dir),
        })
    }

    /// Replace how the backend learns who is calling.
    #[must_use]
    pub fn with_caller(mut self, caller: Arc<dyn Caller>) -> Self {
        self.caller = caller;
        self
    }

    /// Run `hook` after every successful `create_wiki`, with the new
    /// wiki's slug and root. The server uses it to register the
    /// directory as a vault-sync root (`wiki:<slug>`) so the editor
    /// can open its pages immediately.
    #[must_use]
    pub fn with_on_created(mut self, hook: WikiCreatedHook) -> Self {
        self.on_created = Some(hook);
        self
    }

    fn from_layout(layout: Layout) -> Self {
        Self {
            layout,
            caller: Arc::new(GateCaller),
            watch_flags: Arc::new(std::sync::Mutex::new(HashMap::new())),
            changes: architect::PubSub::sliding(256),
            on_created: None,
        }
    }

    /// The account making this call, when the gate knows one.
    #[must_use]
    pub fn calling_principal(&self) -> Option<String> {
        self.caller.principal()
    }

    /// Whether the caller holds the org's admin role.
    #[must_use]
    pub fn caller_is_org_admin(&self) -> bool {
        self.caller.is_org_admin()
    }

    /// Whether the caller is a member of the owning org.
    #[must_use]
    pub fn caller_is_org_member(&self) -> bool {
        self.caller.is_org_member()
    }

    /// Every `(slug, root)` the backend currently serves, sorted by
    /// slug and without the compatibility alias.
    #[must_use]
    pub fn roots(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = match &self.layout {
            Layout::Explicit { roots, .. } => roots
                .read()
                .map(|m| {
                    m.iter()
                        .filter(|(slug, _)| slug.as_str() != COMPAT_WIKI_ID)
                        .map(|(s, p)| (s.clone(), p.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            Layout::UnderParent(parent) => std::fs::read_dir(parent)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path())))
                .collect(),
        };
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The directory a wiki id resolves to, without opening it.
    pub fn root_of(&self, wiki_id: &str) -> Result<PathBuf, WikiError> {
        match &self.layout {
            Layout::Explicit { roots, .. } => roots
                .read()
                .ok()
                .and_then(|m| m.get(wiki_id).cloned())
                .ok_or_else(|| WikiError::WikiNotFound(wiki_id.to_string())),
            Layout::UnderParent(parent) => {
                let root = parent.join(wiki_id);
                root.is_dir()
                    .then_some(root)
                    .ok_or_else(|| WikiError::WikiNotFound(wiki_id.to_string()))
            }
        }
    }

    /// A wiki root as `WikiDescription::root` reports it: relative to
    /// the org root when the backend knows one (the parent of the
    /// `wikis/` directory it creates into), the absolute path otherwise.
    ///
    /// Slashes are normalised so the string means the same thing to a
    /// client on any platform.
    fn org_relative(&self, root: &Path) -> String {
        let org_root = match &self.layout {
            Layout::Explicit {
                wikis_dir: Some(wikis_dir),
                ..
            } => wikis_dir.parent(),
            Layout::Explicit {
                wikis_dir: None, ..
            } => None,
            // `parent/<id>/`: the parent stands in for the org root.
            Layout::UnderParent(parent) => Some(parent.as_path()),
        };
        match org_root.and_then(|org| root.strip_prefix(org).ok()) {
            Some(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
            None => root.display().to_string(),
        }
    }

    /// A wiki's declaration (`_state/wiki.json`), implicit when it has
    /// never written one.
    pub fn config_of(&self, wiki_id: &str) -> Result<WikiConfig, WikiError> {
        let root = self.root_of(wiki_id)?;
        crate::config::load(&root, wiki_id)
    }

    /// Read, change and write a wiki's declaration.
    pub fn update_config(
        &self,
        wiki_id: &str,
        f: impl FnOnce(&mut WikiConfig),
    ) -> Result<WikiConfig, WikiError> {
        let root = self.root_of(wiki_id)?;
        crate::config::update(&root, wiki_id, f)
    }

    /// Announce a committed change to every `changes` subscriber.
    /// Call only *after* the write landed — subscribers use these to
    /// decide what to re-fetch, so a speculative event costs a
    /// pointless round-trip (or worse, shows state that never was).
    pub fn emit(&self, wiki_id: &str, event: WikiEvent) {
        self.changes.publish(WikiChange {
            wiki_id: wiki_id.to_string(),
            event,
        });
    }

    fn resolve(&self, wiki_id: &str) -> Result<WikiLive, WikiError> {
        let root = match &self.layout {
            Layout::Explicit { roots, .. } => roots
                .read()
                .ok()
                .and_then(|m| m.get(wiki_id).cloned())
                .ok_or_else(|| WikiError::WikiNotFound(wiki_id.to_string()))?,
            // The older shape creates on first touch; keeping that
            // here rather than in `root_of` so a listing never invents
            // a wiki by asking about it.
            Layout::UnderParent(parent) => parent.join(wiki_id),
        };
        Ok(WikiLive::open(root))
    }
}

// ────────────────────── Registry ──────────────────────

/// The slug the org's long-standing curated tier answers to.
const DEFAULT_SLUG: &str = "knowledge";

/// The id every client used before an org could hold more than one
/// wiki. Kept as an alias so an older caller still resolves; excluded
/// from listings so it never looks like a second wiki.
pub const COMPAT_WIKI_ID: &str = "default";

impl wiki_proto::service::registry::Registry for WikiBackend {
    /// t[impl wiki.many.addressable] — the org's whole set, so a
    /// client can pick a wiki instead of hard-coding one. This is the
    /// call that exists before a caller has a wiki id, which is why it
    /// takes none.
    fn list_wikis(&self) -> Result<Vec<WikiSummary>, WikiError> {
        Ok(self
            .roots()
            .iter()
            .map(|(slug, root)| summarize(slug, root))
            .collect())
    }

    fn describe_wiki(&self, wiki_id: &str) -> Result<WikiDescription, WikiError> {
        let root = self.root_of(wiki_id)?;
        let config = crate::config::load(&root, wiki_id)?;
        Ok(WikiDescription {
            summary: summarize_with(wiki_id, &root, &config),
            config,
            root: self.org_relative(&root),
        })
    }

    /// t[impl wiki.many.set] — creating a wiki touches its own
    /// directory and the set's index, nothing else; every other wiki
    /// is byte-identical afterwards.
    ///
    /// t[impl wiki.many.identity] — the slug is checked against every
    /// wiki the org holds *and* every slug it has ever retired, so a
    /// reference in someone else's vault can never come to mean a
    /// different wiki.
    fn create_wiki(&self, new: NewWiki) -> Result<WikiSummary, WikiError> {
        let Layout::Explicit {
            roots,
            wikis_dir: Some(wikis_dir),
        } = &self.layout
        else {
            return Err(WikiError::Refused(
                "this backend has no directory to create wikis in".into(),
            ));
        };
        let title = new.title.trim();
        if title.is_empty() {
            return Err(WikiError::Refused("a wiki needs a title".into()));
        }
        let slug = if new.slug.trim().is_empty() {
            wiki_proto::config::slugify(title)
        } else {
            new.slug.trim().to_owned()
        };
        if slug.is_empty() || slug != wiki_proto::config::slugify(&slug) {
            return Err(WikiError::Refused(format!(
                "`{slug}` is not a slug: lowercase words joined by single hyphens"
            )));
        }
        if slug == COMPAT_WIKI_ID {
            return Err(WikiError::Refused(format!(
                "`{slug}` is reserved as an alias of the default wiki"
            )));
        }
        if crate::config::retired(wikis_dir)?.contains(&slug) {
            return Err(WikiError::Refused(format!(
                "`{slug}` was a wiki here once and references may still name it; \
                 a retired slug is never reassigned"
            )));
        }
        let root = wikis_dir.join(&slug);
        {
            let held = roots
                .read()
                .map_err(|_| WikiError::Backend("roots lock".into()))?;
            if held.contains_key(&slug) || root.exists() {
                return Err(WikiError::Refused(format!(
                    "a wiki `{slug}` already exists"
                )));
            }
        }

        std::fs::create_dir_all(&root).map_err(|e| WikiError::Io(e.to_string()))?;
        let live = WikiLive::open(&root);
        // Purpose first, so the bootstrap keeps ours instead of writing
        // the stub. The title rides its frontmatter, which is where
        // `list_wikis` has always read it from.
        let purpose = if new.purpose.trim().is_empty() {
            format!("---\ntitle: \"{title}\"\n---\n\n# {title}\n")
        } else {
            format!(
                "---\ntitle: \"{title}\"\n---\n\n# {title}\n\n{}\n",
                new.purpose.trim()
            )
        };
        std::fs::write(root.join(wiki_proto::paths::PURPOSE_MD), purpose)
            .map_err(|e| WikiError::Io(e.to_string()))?;
        live.bootstrap().map_err(map_err)?;

        // The creator is the first Editor. That is what turns the Edit
        // lane on for this wiki from its first page (`wiki.edit.editor`);
        // an in-process creation — the server planting a seed — has no
        // caller and leaves the lane to whoever grants it.
        let config = WikiConfig {
            slug: slug.clone(),
            title: title.to_owned(),
            visibility: new.visibility,
            editors: self.calling_principal().into_iter().collect(),
            proposers: wiki_proto::config::ProposerGate::default(),
            source: new.source,
            created_at: Utc::now().to_rfc3339(),
        };
        crate::config::save(&root, &config)?;

        // t[impl wiki.source.repo] — a wiki declared over a repository
        // is a mirror of it from its first moment: the first sync runs
        // here, so the pages a creator sees are the repository's. A
        // failed first sync still creates the wiki — it exists, holds
        // no pages yet, and its source says why (`last_error`) — so a
        // typo in the URL is fixed by editing the config rather than by
        // retrying creation against a retired slug.
        let mut config = config;
        if let Some(source) = config.source.as_mut() {
            let _ = blocking(|| crate::repo_source::sync(wikis_dir, &root, source));
            crate::config::save(&root, &config)?;
        }

        roots
            .write()
            .map_err(|_| WikiError::Backend("roots lock".into()))?
            .insert(slug.clone(), root.clone());
        // The wiki exists and is in the set; whoever else serves its
        // directory (the vault-sync path the editor writes through)
        // learns about it now, so it is editable without a restart.
        if let Some(hook) = &self.on_created {
            hook(&slug, &root);
        }
        Ok(summarize_with(&slug, &root, &config))
    }

    /// t[impl wiki.access.visibility] — set per wiki, and read by
    /// every subscriber's resolver on the next attempt, so narrowing
    /// takes effect on what is already published without deleting it.
    fn set_visibility(&self, wiki_id: &str, visibility: Visibility) -> Result<(), WikiError> {
        self.update_config(wiki_id, |c| c.visibility = visibility)?;
        Ok(())
    }

    /// t[impl wiki.many.identity] — retitling changes the title and
    /// nothing a reference or a subscription carries.
    fn set_title(&self, wiki_id: &str, title: &str) -> Result<(), WikiError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(WikiError::Refused("a wiki needs a title".into()));
        }
        let root = self.root_of(wiki_id)?;
        crate::config::update(&root, wiki_id, |c| c.title = title.to_owned())?;
        // Keep `purpose.md`'s frontmatter in step, since a mounted
        // folder shows that and not the JSON.
        let path = root.join(wiki_proto::paths::PURPOSE_MD);
        if let Ok(text) = std::fs::read_to_string(&path) {
            let rewritten = retitle_frontmatter(&text, title);
            std::fs::write(&path, rewritten).map_err(|e| WikiError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// t[impl wiki.many.set] — deleting one wiki removes its directory
    /// and its entry, and leaves every other untouched. The slug is
    /// retired rather than freed (`wiki.many.identity`).
    fn delete_wiki(&self, wiki_id: &str) -> Result<(), WikiError> {
        if wiki_id == COMPAT_WIKI_ID {
            return Err(WikiError::Refused(
                "`default` is an alias; delete the wiki by its own slug".into(),
            ));
        }
        let Layout::Explicit { roots, wikis_dir } = &self.layout else {
            return Err(WikiError::Refused(
                "this backend does not manage its wikis' lifetimes".into(),
            ));
        };
        let root = self.root_of(wiki_id)?;
        if let Some(dir) = wikis_dir {
            crate::config::retire(dir, wiki_id)?;
        }
        std::fs::remove_dir_all(&root).map_err(|e| WikiError::Io(e.to_string()))?;
        roots
            .write()
            .map_err(|_| WikiError::Backend("roots lock".into()))?
            .remove(wiki_id);
        Ok(())
    }

    /// t[impl wiki.source.sync] — fetch the repository and re-export
    /// the followed path now. The source is saved whether the fetch
    /// succeeded or not: on success it names the new commit, on
    /// failure it carries `last_error`, and either way the wiki tells
    /// the truth about what its pages reflect. The outcome rides the
    /// call's span as `wiki.source.outcome`.
    fn refresh_source(&self, wiki_id: &str) -> Result<wiki_proto::config::RepoSource, WikiError> {
        let root = self.root_of(wiki_id)?;
        let mut config = crate::config::load(&root, wiki_id)?;
        let Some(source) = config.source.as_mut() else {
            return Err(WikiError::Refused(format!(
                "`{wiki_id}` has no repository behind it; nothing to refresh"
            )));
        };
        let Layout::Explicit {
            wikis_dir: Some(wikis_dir),
            ..
        } = &self.layout
        else {
            return Err(WikiError::Refused(
                "this backend has no directory to keep a repository clone in".into(),
            ));
        };
        let outcome = blocking(|| crate::repo_source::sync(wikis_dir, &root, source));
        architect_telemetry::wide::set(
            "wiki.source.outcome",
            match &outcome {
                Ok(o) if o.unchanged => "unchanged",
                Ok(_) => "synced",
                Err(_) => "failed",
            },
        );
        let source = source.clone();
        crate::config::save(&root, &config)?;
        match outcome {
            Ok(o) => {
                if !o.unchanged {
                    // Pages moved under a client's feet in bulk; a
                    // re-read is the honest response.
                    self.emit(wiki_id, WikiEvent::Resync);
                }
                Ok(source)
            }
            Err(e) => Err(e),
        }
    }
}

fn summarize(slug: &str, root: &Path) -> WikiSummary {
    let config = crate::config::load(root, slug).unwrap_or_else(|_| WikiConfig::implicit(slug));
    summarize_with(slug, root, &config)
}

fn summarize_with(slug: &str, root: &Path, config: &WikiConfig) -> WikiSummary {
    let title = if config.title.trim().is_empty() {
        title_of(root).unwrap_or_else(|| prettify(slug))
    } else {
        config.title.clone()
    };
    WikiSummary {
        slug: slug.to_owned(),
        title,
        purpose: purpose_of(root),
        visibility: config.visibility,
        pages: count_markdown(root),
        default: slug == DEFAULT_SLUG,
        repo_sourced: config.is_repo_sourced(),
    }
}

/// A wiki's own name for itself, from `purpose.md`'s frontmatter.
fn title_of(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(wiki_proto::paths::PURPOSE_MD)).ok()?;
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

/// The first paragraph of `purpose.md` after its frontmatter and
/// heading — one line of orientation for a picker.
fn purpose_of(root: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(root.join(wiki_proto::paths::PURPOSE_MD)) else {
        return String::new();
    };
    let mut body = text.as_str();
    if let Some(rest) = body.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            body = &rest[end + 4..];
        }
    }
    let mut para = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(line);
    }
    para
}

/// Replace (or add) `title:` in a markdown file's frontmatter.
fn retitle_frontmatter(text: &str, title: &str) -> String {
    let quoted = format!("title: \"{title}\"");
    let Some(rest) = text.strip_prefix("---\n") else {
        return format!("---\n{quoted}\n---\n\n{text}");
    };
    let Some(end) = rest.find("\n---") else {
        return format!("---\n{quoted}\n---\n\n{text}");
    };
    let (front, tail) = rest.split_at(end);
    let mut lines: Vec<String> = front.lines().map(str::to_owned).collect();
    if let Some(l) = lines
        .iter_mut()
        .find(|l| l.trim_start().starts_with("title:"))
    {
        *l = quoted;
    } else {
        lines.insert(0, quoted);
    }
    format!("---\n{}{tail}", lines.join("\n"))
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

fn count_markdown(root: &Path) -> u32 {
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
        let _ = blocking(|| w.rebuild_index()).map_err(map_err)?;
        Ok(ctypes::WikiIndex {
            sections: Vec::new(),
            total: 0,
        })
    }
    fn rebuild_index(&self, wiki_id: &str) -> Result<ctypes::WikiIndex, WikiError> {
        let w = self.resolve(wiki_id)?;
        let _ = blocking(|| w.rebuild_index()).map_err(map_err)?;
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
        blocking(|| w.import_raw_source(source)).map_err(map_err)
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
        let diff = blocking(|| w.rescan_sources()).map_err(map_err)?;
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
        let diff = blocking(|| w.rescan_sources()).map_err(map_err)?;
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

    /// t[impl wiki.edit.editor] — once a wiki has declared Editors,
    /// only they write to it directly; everyone else's change goes
    /// through an Edit Request. An in-process call (no principal — the
    /// server planting a seed, the ingest pipeline) keeps writing: the
    /// lane governs *people*, and the server is not one.
    fn write_page(
        &self,
        wiki_id: &str,
        path: &str,
        markdown: &str,
        base_sha256: &str,
    ) -> Result<ptypes::WikiPageDoc, WikiError> {
        let w = self.resolve(wiki_id)?;
        let rel = sanitize_page_path(path)?;
        if let Some(principal) = self.calling_principal() {
            let config = self.config_of(wiki_id)?;
            if config.has_edit_lane() && !config.is_editor(&principal) {
                return Err(WikiError::Refused(format!(
                    "`{wiki_id}` is governed by its Editors; open an Edit Request to change `{rel}`"
                )));
            }
        }
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

/// Whether a wiki-root-relative path is a curated page — a `.md` file
/// outside the raw / state / media subtrees. What a writer that
/// bypasses `write_page` (the vault-sync editor path, a disk edit)
/// should check before announcing a `PageWritten`.
#[must_use]
pub fn is_curated_page_path(rel: &str) -> bool {
    sanitize_page_path(rel).is_ok()
}

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
        blocking(|| wiki_graph::build_graph(w.vault_root(), opts))
            .map_err(|e| WikiError::Backend(e.to_string()))
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
        blocking(|| wiki_graph::build_clusters(w.vault_root()))
            .map_err(|e| WikiError::Backend(e.to_string()))
    }
    fn gaps(&self, wiki_id: &str) -> Result<Vec<gtypes::KnowledgeGap>, WikiError> {
        let w = self.resolve(wiki_id)?;
        blocking(|| wiki_graph::find_gaps(w.vault_root()))
            .map_err(|e| WikiError::Backend(e.to_string()))
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
        blocking(|| wiki_search::search(w.vault_root(), opts))
            .map_err(|e| WikiError::Backend(e.to_string()))
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

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
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
        blocking(|| wiki_extract::extract_path(&abs, &opts))
            .map_err(|e| WikiError::Backend(format!("extract: {e}")))
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use wiki_proto::service::registry::Registry;

    struct Alice;
    impl Caller for Alice {
        fn principal(&self) -> Option<String> {
            Some("alice".into())
        }
    }

    fn org(dir: &Path) -> WikiBackend {
        let wikis = dir.join("wikis");
        std::fs::create_dir_all(&wikis).unwrap();
        WikiBackend::with_roots_under(HashMap::new(), wikis).with_caller(Arc::new(Alice))
    }

    /// A description says where the wiki lives, relative to the org
    /// root, for both shapes an org holds: the default tier under
    /// `wiki/Knowledge` and a created wiki under `wikis/<slug>`.
    #[test]
    fn describe_reports_the_root_relative_to_the_org() {
        let dir = tempfile::tempdir().unwrap();
        let knowledge = dir.path().join("wiki").join("Knowledge");
        std::fs::create_dir_all(&knowledge).unwrap();
        let mut roots = HashMap::new();
        roots.insert("knowledge".to_owned(), knowledge);
        let b = WikiBackend::with_roots_under(roots, dir.path().join("wikis"))
            .with_caller(Arc::new(Alice));
        b.create_wiki(NewWiki {
            title: "Bible Study".into(),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(b.describe_wiki("knowledge").unwrap().root, "wiki/Knowledge");
        assert_eq!(
            b.describe_wiki("bible-study").unwrap().root,
            "wikis/bible-study"
        );

        // With nowhere to be relative to, the path is given whole
        // rather than invented.
        let single = WikiBackend::single("solo", dir.path().join("solo")).unwrap();
        assert_eq!(
            single.describe_wiki("solo").unwrap().root,
            dir.path().join("solo").display().to_string()
        );
    }

    /// t[verify wiki.many.set] — an org with none is legal; creating
    /// one leaves the others byte-identical; deleting one leaves the
    /// rest.
    #[test]
    fn the_set_grows_and_shrinks_one_wiki_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let b = org(dir.path());
        assert!(b.list_wikis().unwrap().is_empty());

        let theory = b
            .create_wiki(NewWiki {
                title: "Music Theory".into(),
                purpose: "Intervals, scales, harmony.".into(),
                visibility: Visibility::Public,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(theory.slug, "music-theory");
        assert_eq!(theory.title, "Music Theory");
        assert_eq!(theory.purpose, "Intervals, scales, harmony.");
        assert_eq!(theory.visibility, Visibility::Public);

        let before = snapshot(&dir.path().join("wikis/music-theory"));
        b.create_wiki(NewWiki {
            title: "Audio Production".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            snapshot(&dir.path().join("wikis/music-theory")),
            before,
            "creating a second wiki changed the first"
        );
        let slugs: Vec<String> = b
            .list_wikis()
            .unwrap()
            .into_iter()
            .map(|w| w.slug)
            .collect();
        assert_eq!(slugs, vec!["audio-production", "music-theory"]);

        b.delete_wiki("audio-production").unwrap();
        let slugs: Vec<String> = b
            .list_wikis()
            .unwrap()
            .into_iter()
            .map(|w| w.slug)
            .collect();
        assert_eq!(slugs, vec!["music-theory"]);
        assert_eq!(snapshot(&dir.path().join("wikis/music-theory")), before);
    }

    /// t[verify wiki.many.identity] — a retitle changes no slug; a
    /// deleted wiki's slug is refused forever; two wikis never share
    /// one.
    #[test]
    fn a_slug_is_stable_unique_and_never_reassigned() {
        let dir = tempfile::tempdir().unwrap();
        let b = org(dir.path());
        b.create_wiki(NewWiki {
            title: "Cooking".into(),
            ..Default::default()
        })
        .unwrap();
        b.set_title("cooking", "Kitchen Notes").unwrap();
        let d = b.describe_wiki("cooking").unwrap();
        assert_eq!(d.summary.slug, "cooking");
        assert_eq!(d.summary.title, "Kitchen Notes");
        assert!(
            std::fs::read_to_string(dir.path().join("wikis/cooking/purpose.md"))
                .unwrap()
                .contains("title: \"Kitchen Notes\""),
            "the mounted folder shows the new title too"
        );

        let dup = b.create_wiki(NewWiki {
            title: "cooking".into(),
            ..Default::default()
        });
        assert!(matches!(dup, Err(WikiError::Refused(_))), "{dup:?}");

        b.delete_wiki("cooking").unwrap();
        let again = b.create_wiki(NewWiki {
            title: "Cooking".into(),
            ..Default::default()
        });
        assert!(
            matches!(again, Err(WikiError::Refused(ref m)) if m.contains("retired")),
            "{again:?}"
        );
    }

    /// The creator holds Editor from the first page, so the lane
    /// governs a new wiki without a second step.
    #[test]
    fn the_creator_is_the_first_editor() {
        let dir = tempfile::tempdir().unwrap();
        let b = org(dir.path());
        b.create_wiki(NewWiki {
            title: "Bible Study".into(),
            ..Default::default()
        })
        .unwrap();
        let c = b.config_of("bible-study").unwrap();
        assert_eq!(c.editors, vec!["alice".to_string()]);
        assert!(c.has_edit_lane());
        assert_eq!(c.visibility, Visibility::Private, "private until promoted");
    }

    #[test]
    fn retitle_frontmatter_replaces_or_adds() {
        assert_eq!(
            retitle_frontmatter("---\ntitle: \"Old\"\nx: 1\n---\n\nbody", "New"),
            "---\ntitle: \"New\"\nx: 1\n---\n\nbody"
        );
        assert_eq!(
            retitle_frontmatter("---\nx: 1\n---\nbody", "New"),
            "---\ntitle: \"New\"\nx: 1\n---\nbody"
        );
        assert_eq!(
            retitle_frontmatter("body", "New"),
            "---\ntitle: \"New\"\n---\n\nbody"
        );
    }

    /// t[verify wiki.source.repo] — a wiki created over a repository
    /// holds the repository's pages the moment it exists, says which
    /// commit, and lists as repo-sourced; one created over a URL that
    /// does not answer still exists, empty, and says why.
    #[test]
    fn a_repo_sourced_wiki_mirrors_on_creation_and_refreshes() {
        fn g(dir: &Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .args(["-c", "user.email=t@example.com", "-c", "user.name=T"])
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::create_dir_all(work.join("docs")).unwrap();
        g(&bare, &["init", "--bare", "--initial-branch=main"]);
        g(&work, &["init", "--initial-branch=main"]);
        std::fs::write(work.join("docs/Getting Started.md"), "# Getting Started\n").unwrap();
        g(&work, &["add", "-A"]);
        g(&work, &["commit", "-m", "docs"]);
        g(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        g(&work, &["push", "-u", "origin", "main"]);

        let b = org(dir.path());
        let source = wiki_proto::config::RepoSource {
            url: format!("file://{}", bare.display()),
            branch: "main".into(),
            path: "docs".into(),
            ..Default::default()
        };
        let summary = b
            .create_wiki(NewWiki {
                title: "Docs".into(),
                visibility: Visibility::Public,
                source: Some(source.clone()),
                ..Default::default()
            })
            .unwrap();
        assert!(summary.repo_sourced);
        let d = b.describe_wiki("docs").unwrap();
        let got = d.config.source.expect("source kept");
        assert_eq!(got.commit.len(), 40, "{got:?}");
        assert!(got.last_error.is_empty(), "{got:?}");
        assert!(dir.path().join("wikis/docs/Getting Started.md").is_file());
        assert!(
            dir.path().join("wikis/docs/purpose.md").is_file(),
            "the scaffold is kept"
        );
        let slugs: Vec<String> = b
            .list_wikis()
            .unwrap()
            .into_iter()
            .map(|w| w.slug)
            .collect();
        assert_eq!(
            slugs,
            vec!["docs"],
            "the clone under `.repos/` is not a wiki"
        );

        // Upstream moves; a refresh follows it.
        std::fs::write(work.join("docs/Deploying.md"), "# Deploying\n").unwrap();
        g(&work, &["add", "-A"]);
        g(&work, &["commit", "-m", "more"]);
        g(&work, &["push", "origin", "main"]);
        let after = b.refresh_source("docs").unwrap();
        assert_ne!(after.commit, got.commit);
        assert!(dir.path().join("wikis/docs/Deploying.md").is_file());

        // A wiki with no repository refuses to refresh.
        b.create_wiki(NewWiki {
            title: "Plain".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(matches!(
            b.refresh_source("plain"),
            Err(WikiError::Refused(_))
        ));

        // A dead URL: the wiki exists and says it is stale.
        b.create_wiki(NewWiki {
            title: "Broken".into(),
            source: Some(wiki_proto::config::RepoSource {
                url: format!("file://{}", dir.path().join("nowhere.git").display()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        let broken = b.config_of("broken").unwrap().source.unwrap();
        assert!(broken.commit.is_empty());
        assert!(!broken.last_error.is_empty(), "{broken:?}");
        assert!(matches!(b.refresh_source("broken"), Err(WikiError::Io(_))));
    }

    fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for e in walkdir::WalkDir::new(root).into_iter().flatten() {
            if e.file_type().is_file() {
                out.push((
                    e.path().strip_prefix(root).unwrap().display().to_string(),
                    std::fs::read(e.path()).unwrap(),
                ));
            }
        }
        out.sort();
        out
    }
}
