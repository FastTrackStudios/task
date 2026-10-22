//! What a grant on a project exposes — the in-scope set behind every
//! preview and every guest call, and the digest an owner confirms it
//! with.
//!
//! # The rule
//!
//! `t[sharing.grant.subtree]`: a grant on project P reaches
//!
//! - P and its sub-projects at any depth (`t[project.nesting.uniform]` —
//!   nesting is the declared `parent_id` link, never a directory name);
//! - the parts of any subtree project (and a promoted part keeps its id,
//!   so it stays in scope as the sub-project it became);
//! - the tasks whose `project_id` is a subtree project or part — a task
//!   with no project, or with a project outside the subtree, is out;
//! - the milestones and workstreams attached to subtree projects (goals
//!   are org-level and always out);
//! - the File Roots adopted from the subtree projects' directories, as
//!   root ids plus path rules within each root (for the files lane);
//! - the notes under the subtree projects' directories on the Projects
//!   tier, as path rules (for the vault lane);
//! - the threads anchored on an in-scope object.
//!
//! Sibling and parent projects are never in scope, and neither is
//! anything `t[sharing.grant.exclusions]` names — money (timer, finance,
//! `client_id`, `default_rate_cents`, budget), other projects, contacts,
//! org-level feeds. The resolver yields object identities only; it never
//! carries a project's fields, so nothing money-shaped can ride along.
//! A setlist part that *references* a song owned elsewhere is in scope as
//! a roster entry; the song it points at is not.
//!
//! # Evaluated per call
//!
//! Captain decision 5: the subtree is automatic. [`resolve`] is a pure
//! function over the owner's current state, and [`OwnerStore`] reads that
//! state fresh on every call — there is no snapshot taken at grant time
//! and no cache, so a song added to Crescendum tomorrow is shared with no
//! new grant, and a sub-project moved out today is unshared at once.
//!
//! # Directories are not parentage
//!
//! A project's directory says where its material is; its `parent_id`
//! says whose it is. When a project that is NOT in the subtree has its
//! directory inside one that is (a sub-project moved out of Crescendum
//! whose files never moved), that directory is carved out: path rules
//! match by longest prefix, and the carve-out is the longer one.
//!
//! # The digest
//!
//! One function, [`digest`], over the visible set's `(kind, id)` pairs.
//! Labels are left out on purpose: renaming a song is not a change in
//! what is shared, and must not make a confirmed preview stale. The
//! preview and the lane both call [`Subtree::scope`], so the set the
//! owner confirmed and the set the guest reaches are the same function
//! (`t[sharing.grant.preview]`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use share_proto::VisibleItem;
use uuid::Uuid;

/// Computes the in-scope set of a grant rooted at a project.
pub trait Subtree: Send + Sync + 'static {
    /// Everything a grant on `root` reaches right now, or `None` when
    /// there is no such project (or the owner's state cannot be read) —
    /// which every caller answers as absent. Called on a blocking
    /// thread: implementations may walk the disk.
    fn scope(&self, root: Uuid) -> Option<InScope>;

    /// The preview's rendering of [`Self::scope`], in canonical order.
    fn visible(&self, root: Uuid) -> Option<Vec<VisibleItem>> {
        self.scope(root).map(|s| s.visible())
    }
}

// ── the owner's state, as the pure resolver sees it ──────────────────

/// One project, reduced to what scope needs: identity, parentage, where
/// its page sits, and its parts. No other field of the project is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub title: String,
    /// The page's path on the Projects tier (`crescendum/project.md`).
    pub page: String,
    /// `(part id, name)` — the project's roster.
    pub parts: Vec<(Uuid, String)>,
}

/// Something attached to a project by id: a task, milestone or
/// workstream. `project_id` is `None` for a task filed nowhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
}

/// A registered File Root and its tree on this host, if it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootNode {
    pub id: Uuid,
    pub name: String,
    pub tree: Option<PathBuf>,
}

/// The owner org's state the resolver reads. Built fresh per call.
#[derive(Clone, Debug, Default)]
pub struct OwnerState {
    pub projects: Vec<ProjectNode>,
    pub tasks: Vec<Attached>,
    pub milestones: Vec<Attached>,
    pub workstreams: Vec<Attached>,
    pub roots: Vec<RootNode>,
    /// The Projects tier on this host, so a root's tree can be related
    /// to a project's directory. `None` relates no root to anything.
    pub projects_dir: Option<PathBuf>,
}

// ── what can be asked about ──────────────────────────────────────────

/// Something a guest call names, to be checked with [`InScope::in_scope`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectRef {
    Project(Uuid),
    Part(Uuid),
    Task(Uuid),
    Milestone(Uuid),
    Workstream(Uuid),
    /// Goals are org-level: never in scope.
    Goal(Uuid),
    /// A path inside a File Root, relative to the root.
    File {
        root: Uuid,
        path: String,
    },
    /// A note, by its path on the Projects tier (`crescendum/notes.md`).
    Note(String),
    /// A thread, by the object it is anchored on (`Thread::entity_type`,
    /// `Thread::entity_id`).
    Thread {
        entity_type: String,
        entity_id: Uuid,
    },
}

/// Path rules within one tree: the longest matching prefix decides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathRules {
    /// `(prefix, included)`; a prefix is a `/`-joined relative directory
    /// with no trailing slash, `""` meaning the whole tree.
    rules: BTreeMap<String, bool>,
}

impl PathRules {
    /// Callers insert carve-outs first and inclusions second, so when
    /// hand-edited pages put a subtree project and an outside one in the
    /// same directory, the subtree project's claim on it stands.
    fn set(&mut self, prefix: String, included: bool) {
        self.rules.insert(prefix, included);
    }

    /// Whether `path` (relative, `/`-separated) falls inside the rules.
    /// A path with a `..` component, or an absolute one, never does.
    #[must_use]
    pub fn contains(&self, path: &str) -> bool {
        let Some(path) = normalise(path) else {
            return false;
        };
        let mut best: Option<(usize, bool)> = None;
        for (prefix, &included) in &self.rules {
            if under(&path, prefix) && best.is_none_or(|(len, _)| prefix.len() >= len) {
                best = Some((prefix.len(), included));
            }
        }
        best.is_some_and(|(_, included)| included)
    }

    /// The included prefixes, in order.
    pub fn included(&self) -> impl Iterator<Item = &str> {
        self.rules
            .iter()
            .filter(|(_, inc)| **inc)
            .map(|(p, _)| p.as_str())
    }

    /// The carved-out prefixes, in order.
    pub fn excluded(&self) -> impl Iterator<Item = &str> {
        self.rules
            .iter()
            .filter(|(_, inc)| !**inc)
            .map(|(p, _)| p.as_str())
    }

    fn any_included(&self) -> bool {
        self.rules.values().any(|inc| *inc)
    }
}

/// One File Root a grant reaches, and where inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRootScope {
    pub root_id: Uuid,
    pub name: String,
    pub paths: PathRules,
}

/// The resolved in-scope set of one grant root.
// t[impl sharing.grant.subtree]
// t[impl sharing.grant.exclusions]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InScope {
    pub root: Uuid,
    /// Subtree projects: id → title. Includes the root.
    pub projects: BTreeMap<Uuid, String>,
    /// Parts of subtree projects: id → name.
    pub parts: BTreeMap<Uuid, String>,
    pub tasks: BTreeMap<Uuid, String>,
    pub milestones: BTreeMap<Uuid, String>,
    pub workstreams: BTreeMap<Uuid, String>,
    /// By root id.
    pub file_roots: BTreeMap<Uuid, FileRootScope>,
    /// Note paths on the Projects tier.
    pub notes: PathRules,
}

impl InScope {
    /// THE membership predicate: whether a guest call may reach `object`.
    /// Anything it answers `false` for reads as absent
    /// (`t[sharing.grant.absent]`).
    #[must_use]
    pub fn in_scope(&self, object: &ObjectRef) -> bool {
        match object {
            ObjectRef::Project(id) => self.projects.contains_key(id),
            // A part keeps its id through promotion and demotion
            // (`project.part.promotion`), so a link to the part resolves
            // on both sides of the line.
            ObjectRef::Part(id) => self.parts.contains_key(id) || self.projects.contains_key(id),
            ObjectRef::Task(id) => self.tasks.contains_key(id),
            ObjectRef::Milestone(id) => self.milestones.contains_key(id),
            ObjectRef::Workstream(id) => self.workstreams.contains_key(id),
            ObjectRef::Goal(_) => false,
            ObjectRef::File { root, path } => self
                .file_roots
                .get(root)
                .is_some_and(|r| r.paths.contains(path)),
            ObjectRef::Note(path) => self.notes.contains(path),
            ObjectRef::Thread {
                entity_type,
                entity_id,
            } => thread_anchor(entity_type, *entity_id).is_some_and(|o| self.in_scope(&o)),
        }
    }

    /// The set in its canonical `(kind, id)` order — what the preview
    /// shows and [`digest`] is computed over. Threads are not listed:
    /// they are content of the objects listed, admitted by their anchor.
    #[must_use]
    pub fn visible(&self) -> Vec<VisibleItem> {
        let mut out = Vec::new();
        let mut push = |kind: &str, id: String, label: String| {
            out.push(VisibleItem {
                kind: kind.into(),
                id,
                label,
            });
        };
        for (id, t) in &self.projects {
            push("project", id.to_string(), t.clone());
        }
        for (id, t) in &self.parts {
            push("part", id.to_string(), t.clone());
        }
        for (id, t) in &self.tasks {
            push("task", id.to_string(), t.clone());
        }
        for (id, t) in &self.milestones {
            push("milestone", id.to_string(), t.clone());
        }
        for (id, t) in &self.workstreams {
            push("workstream", id.to_string(), t.clone());
        }
        for (id, r) in &self.file_roots {
            for p in r.paths.included() {
                push("file_root", rule_id(*id, p), labelled(&r.name, p));
            }
            for p in r.paths.excluded() {
                push("file_root_excluded", rule_id(*id, p), labelled(&r.name, p));
            }
        }
        for p in self.notes.included() {
            push("note_dir", format!("{p}/"), format!("{p}/"));
        }
        for p in self.notes.excluded() {
            push("note_dir_excluded", format!("{p}/"), format!("{p}/"));
        }
        sorted(out)
    }
}

fn rule_id(root: Uuid, prefix: &str) -> String {
    if prefix.is_empty() {
        root.to_string()
    } else {
        format!("{root}:{prefix}")
    }
}

fn labelled(name: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{name}/{prefix}")
    }
}

/// The object a thread is anchored on, for the kinds a grant can reach.
/// Any other anchor (a forge issue, an attachment, …) is out.
#[must_use]
pub fn thread_anchor(entity_type: &str, entity_id: Uuid) -> Option<ObjectRef> {
    Some(match entity_type {
        "project" => ObjectRef::Project(entity_id),
        "part" => ObjectRef::Part(entity_id),
        "task" => ObjectRef::Task(entity_id),
        "milestone" => ObjectRef::Milestone(entity_id),
        "workstream" => ObjectRef::Workstream(entity_id),
        _ => return None,
    })
}

// ── the pure resolver ────────────────────────────────────────────────

/// The in-scope set of a grant on `root` over `state`, or `None` when
/// `root` is not a project. Pure: same state, same answer.
// t[impl sharing.grant.subtree]
#[must_use]
pub fn resolve(state: &OwnerState, root: Uuid) -> Option<InScope> {
    let by_id: HashMap<Uuid, &ProjectNode> = state.projects.iter().map(|p| (p.id, p)).collect();
    let root_node = by_id.get(&root)?;
    let mut children: HashMap<Uuid, Vec<&ProjectNode>> = HashMap::new();
    for p in &state.projects {
        if let Some(parent) = p.parent_id {
            children.entry(parent).or_default().push(p);
        }
    }

    // Projects: the root, then everything whose declared parent chain
    // reaches it. A parent cycle in hand-edited frontmatter must not loop.
    let mut projects = BTreeMap::from([(root, root_node.title.clone())]);
    let mut stack = vec![root];
    while let Some(at) = stack.pop() {
        for child in children.get(&at).into_iter().flatten() {
            if let std::collections::btree_map::Entry::Vacant(e) = projects.entry(child.id) {
                e.insert(child.title.clone());
                stack.push(child.id);
            }
        }
    }

    let mut parts = BTreeMap::new();
    for id in projects.keys() {
        for (part, name) in &by_id[id].parts {
            parts.insert(*part, name.clone());
        }
    }

    // A task filed under a part is filed under the subtree: the part is
    // the piece of work, and promoting it would make it a project anyway.
    let anchored = |a: &Attached| {
        a.project_id
            .is_some_and(|p| projects.contains_key(&p) || parts.contains_key(&p))
    };
    let pick = |items: &[Attached]| -> BTreeMap<Uuid, String> {
        items
            .iter()
            .filter(|a| anchored(a))
            .map(|a| (a.id, a.title.clone()))
            .collect()
    };
    let tasks = pick(&state.tasks);
    let milestones = pick(&state.milestones);
    let workstreams = pick(&state.workstreams);

    // Directories on the Projects tier: the subtree's are included, any
    // other project's that sits under one of them is carved out.
    let dir_of = |p: &ProjectNode| page_dir(&p.page);
    let included: BTreeSet<String> = projects.keys().filter_map(|id| dir_of(by_id[id])).collect();
    let excluded = carve_outs(
        state
            .projects
            .iter()
            .filter(|p| !projects.contains_key(&p.id))
            .filter_map(dir_of)
            .filter(|d| included.iter().any(|inc| under(d, inc))),
        &included,
    );
    let mut notes = PathRules::default();
    for d in &excluded {
        notes.set(d.clone(), false);
    }
    for d in &included {
        notes.set(d.clone(), true);
    }

    let file_roots = state
        .projects_dir
        .as_deref()
        .map(|tier| root_scopes(&state.roots, tier, &included, &excluded))
        .unwrap_or_default();

    Some(InScope {
        root,
        projects,
        parts,
        tasks,
        milestones,
        workstreams,
        file_roots,
        notes,
    })
}

/// The carve-outs that change an answer: an outside project's directory
/// under a subtree directory, unless the nearest rule above it is already
/// a carve-out (a moved-out song's own sub-projects add nothing) or the
/// subtree claims the very same directory (inclusion wins there).
fn carve_outs(
    candidates: impl Iterator<Item = String>,
    included: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut candidates: Vec<String> = candidates.filter(|d| !included.contains(d)).collect();
    candidates.sort_by(|a, b| (a.len(), a).cmp(&(b.len(), b)));
    candidates.dedup();
    let mut kept = BTreeSet::new();
    for d in candidates {
        let nearest = included
            .iter()
            .map(|p| (p, true))
            .chain(kept.iter().map(|p| (p, false)))
            .filter(|(p, _)| *p != &d && under(&d, p))
            .max_by_key(|(p, _)| p.len());
        if nearest.is_none_or(|(_, inc)| inc) {
            kept.insert(d);
        }
    }
    kept
}

/// Relate each File Root's tree to the tier directories: a directory
/// inside the root becomes a rule at its path within the root, and the
/// nearest directory that contains the root decides the whole root.
fn root_scopes(
    roots: &[RootNode],
    tier: &Path,
    included: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> BTreeMap<Uuid, FileRootScope> {
    let mut out = BTreeMap::new();
    for root in roots {
        let Some(tree) = root.tree.as_deref() else {
            continue;
        };
        // The root's tree as a tier-relative directory; a root outside
        // the tier is related to nothing.
        let Ok(rel) = tree.strip_prefix(tier) else {
            continue;
        };
        let Some(rel) = rel.to_str().and_then(normalise) else {
            continue;
        };
        let mut paths = PathRules::default();
        // The nearest enclosing directory (longest), if any, covers the
        // root's top.
        let enclosing = included
            .iter()
            .map(|d| (d, true))
            .chain(excluded.iter().map(|d| (d, false)))
            .filter(|(d, _)| under(&rel, d))
            .max_by_key(|(d, inc)| (d.len(), *inc));
        if let Some((_, inc)) = enclosing {
            paths.set(String::new(), inc);
        }
        for d in excluded {
            if let Some(inner) = within(d, &rel) {
                paths.set(inner, false);
            }
        }
        for d in included {
            if let Some(inner) = within(d, &rel) {
                paths.set(inner, true);
            }
        }
        if paths.any_included() {
            out.insert(
                root.id,
                FileRootScope {
                    root_id: root.id,
                    name: root.name.clone(),
                    paths,
                },
            );
        }
    }
    out
}

/// The directory a project page sits in — `crescendum/project.md` →
/// `crescendum`. `None` for a page at the tier root, which owns no
/// directory and so carries no files or notes.
fn page_dir(page: &str) -> Option<String> {
    let page = normalise(page)?;
    page.rsplit_once('/').map(|(dir, _)| dir.to_owned())
}

/// `path` as clean relative components joined by `/`; `None` when it
/// climbs (`..`) or is absolute.
fn normalise(path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('\\') {
        return None;
    }
    let mut parts = Vec::new();
    for c in path.split(['/', '\\']) {
        match c {
            "" | "." => {}
            ".." => return None,
            c => parts.push(c),
        }
    }
    Some(parts.join("/"))
}

/// Whether `path` is `dir` or inside it (component-wise; `""` is the top).
fn under(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path == dir
        || (path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/')
}

/// `dir` relative to `base`, when `dir` is `base` or inside it.
fn within(dir: &str, base: &str) -> Option<String> {
    if !under(dir, base) {
        return None;
    }
    Some(if base.is_empty() {
        dir.to_owned()
    } else {
        dir[base.len()..].trim_start_matches('/').to_owned()
    })
}

// ── the store-backed adapter ─────────────────────────────────────────

/// The owner org's live stores, read on every call. No cache: each
/// [`Subtree::scope`] lists projects, tasks, milestones, workstreams and
/// File Roots afresh, so project, part and task mutations need no
/// invalidation (Captain decision 5).
#[derive(Clone)]
pub struct OwnerStore {
    projects: project::ProjectBackend,
    tasks: Option<task::TaskBackend>,
    milestones: Option<milestone::MilestoneBackend>,
    workstreams: Option<workstream::WorkstreamBackend>,
    files: Option<files::FilesBackend>,
    projects_dir: PathBuf,
}

impl OwnerStore {
    /// Projects only; add the rest with the `with_*` builders.
    #[must_use]
    pub fn new(projects: project::ProjectBackend, projects_dir: PathBuf) -> Self {
        Self {
            projects,
            tasks: None,
            milestones: None,
            workstreams: None,
            files: None,
            projects_dir,
        }
    }

    #[must_use]
    pub fn with_tasks(mut self, tasks: task::TaskBackend) -> Self {
        self.tasks = Some(tasks);
        self
    }

    #[must_use]
    pub fn with_milestones(mut self, m: milestone::MilestoneBackend) -> Self {
        self.milestones = Some(m);
        self
    }

    #[must_use]
    pub fn with_workstreams(mut self, w: workstream::WorkstreamBackend) -> Self {
        self.workstreams = Some(w);
        self
    }

    #[must_use]
    pub fn with_files(mut self, f: files::FilesBackend) -> Self {
        self.files = Some(f);
        self
    }

    /// The owner's state right now. `None` when any wired store cannot
    /// be read: a partial read could omit a carve-out or a project, so
    /// the resolver fails closed and the call reads as absent.
    #[must_use]
    pub fn state(&self) -> Option<OwnerState> {
        use milestone::MilestoneService as _;
        use project::ProjectService as _;
        use task::TaskService as _;
        use workstream::WorkstreamService as _;

        let projects = read("projects", self.projects.list())?
            .into_iter()
            .map(|p| ProjectNode {
                id: p.id,
                parent_id: p.parent_id,
                title: p.title,
                page: p.path,
                parts: p.parts.0.into_iter().map(|x| (x.id, x.name)).collect(),
            })
            .collect();
        let tasks = match &self.tasks {
            Some(t) => read("tasks", t.list())?
                .into_iter()
                .map(|t| Attached {
                    id: t.id,
                    project_id: t.project_id,
                    title: t.title,
                })
                .collect(),
            None => Vec::new(),
        };
        let milestones = match &self.milestones {
            Some(m) => read("milestones", m.list())?
                .into_iter()
                .map(|m| Attached {
                    id: m.id,
                    project_id: Some(m.project_id),
                    title: m.title,
                })
                .collect(),
            None => Vec::new(),
        };
        let workstreams = match &self.workstreams {
            Some(w) => read("workstreams", w.list(None))?
                .into_iter()
                .map(|w| Attached {
                    id: w.id,
                    project_id: Some(w.project_id),
                    title: w.title,
                })
                .collect(),
            None => Vec::new(),
        };
        let roots = self
            .files
            .as_ref()
            .map(|f| {
                f.registered_roots()
                    .into_iter()
                    .map(|r| RootNode {
                        id: r.id,
                        tree: r.local_tree().map(canonical),
                        name: r.name,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(OwnerState {
            projects,
            tasks,
            milestones,
            workstreams,
            roots,
            // Roots are registered canonicalised; relate like to like.
            projects_dir: Some(canonical(&self.projects_dir)),
        })
    }
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn read<T, E: std::fmt::Display>(what: &str, r: Result<T, E>) -> Option<T> {
    r.map_err(|e| {
        tracing::warn!(target: "task_server::sharing", store = what, error = %e, "subtree: owner store unreadable; scope fails closed");
    })
    .ok()
}

impl Subtree for OwnerStore {
    fn scope(&self, root: Uuid) -> Option<InScope> {
        resolve(&self.state()?, root)
    }
}

/// The set in its canonical order: by `(kind, id)`.
#[must_use]
pub fn sorted(mut items: Vec<VisibleItem>) -> Vec<VisibleItem> {
    items.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    items.dedup_by(|a, b| a.kind == b.kind && a.id == b.id);
    items
}

/// THE preview digest: lower-case hex SHA-256 over the root and the
/// visible set's `(kind, id)` pairs in canonical order. Order-independent
/// in its input, label-independent, and the only place the digest is
/// computed — the preview shows it and the create checks against it.
// t[impl sharing.grant.preview]
#[must_use]
pub fn digest(root: Uuid, visible: &[VisibleItem]) -> String {
    let mut pairs: Vec<(&str, &str)> = visible
        .iter()
        .map(|v| (v.kind.as_str(), v.id.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    let mut h = Sha256::new();
    h.update(b"task.sharing.preview.v1\n");
    h.update(root.to_string().as_bytes());
    h.update(b"\n");
    for (kind, id) in pairs {
        h.update(kind.as_bytes());
        h.update(b"\t");
        h.update(id.as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
