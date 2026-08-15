//! `git-config` — persisted bindings between Task entities and
//! forge entities.
//!
//! - [`RepoBinding`] — `project_id` <-> `RepoId`. One project
//!   may bind to N repos; one repo to N projects.
//! - [`IssueLink`] — `task_id` <-> `(RepoId, number, kind)`.
//!   One task may link to N issues/PRs.
//!
//! First pass ships types + an in-memory [`Store`] so the rest
//! of the feature can build and integrate. `SQLite` persistence
//! lands next (matching `email-config`'s store).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use facet::Facet;
use git_proto::RepoId;
use serde::{Deserialize, Serialize};

pub mod sync;
pub use sync::{
    Field, FieldProvenance, FieldSource, ForgeUpdate, Resolution, SyncedFields, forge_update,
    reconcile_synced, resolve_conflict, resolve_field,
};

/// Whether an `IssueLink` points at an issue or a pull request.
/// Forge-level numbers may share a namespace (GitHub) or not
/// (Forgejo) — record the kind so lookups know which trait to
/// route to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum LinkKind {
    Issue,
    Pull,
}

/// One project <-> one forge repo. Many of these per project
/// and per repo are allowed.
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct RepoBinding {
    /// Task-side identifier — currently the project markdown
    /// page's stable id. Opaque to this crate.
    pub project_id: String,
    pub repo: RepoId,
}

/// One task <-> one issue or pull request.
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct IssueLink {
    pub task_id: String,
    pub repo: RepoId,
    pub number: u64,
    pub kind: LinkKind,
}

/// The last-converged baseline for a linked issue's synced fields —
/// what each side held at the previous reconcile. The sync path
/// value-diffs the current values against this to decide, per field,
/// whether the Task side, the forge side, or neither changed; that
/// drives [`sync::reconcile_synced`]. Persisted per `(repo, number)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSnapshot {
    /// Values the Task side held at the last reconcile.
    pub task: SyncedFields,
    /// Values the forge side held at the last reconcile.
    pub forge: SyncedFields,
    /// The forge issue's `updated_at` at the last reconcile, if the
    /// backend surfaced one. Lets the sync fast-path skip an issue
    /// whose forge timestamp hasn't advanced. `#[serde(default)]` so
    /// snapshots written before this field round-trip as `None`.
    #[serde(default)]
    pub forge_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("storage error: {0}")]
    Storage(String),
}

/// Persisted binding store. First-pass impl is in-memory; the
/// trait surface is what callers depend on, so swapping in a
/// SQLite-backed implementation later is mechanical.
pub trait BindingStore: Send + Sync {
    fn add_repo_binding(&self, binding: RepoBinding) -> Result<(), ConfigError>;
    fn remove_repo_binding(&self, project_id: &str, repo: &RepoId) -> Result<(), ConfigError>;
    fn repos_for_project(&self, project_id: &str) -> Result<Vec<RepoId>, ConfigError>;
    fn projects_for_repo(&self, repo: &RepoId) -> Result<Vec<String>, ConfigError>;

    fn add_issue_link(&self, link: IssueLink) -> Result<(), ConfigError>;
    fn remove_issue_link(&self, task_id: &str, number: u64) -> Result<(), ConfigError>;
    fn issues_for_task(&self, task_id: &str) -> Result<Vec<IssueLink>, ConfigError>;
    fn tasks_for_issue(&self, repo: &RepoId, number: u64) -> Result<Vec<String>, ConfigError>;

    /// The last-converged sync baseline for `(repo, number)`, if one
    /// has been recorded. `None` means "never reconciled" — the first
    /// sync treats the forge as authoritative for the forge-owned
    /// fields and then records a snapshot.
    fn get_issue_snapshot(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<Option<IssueSnapshot>, ConfigError>;

    /// Record (replacing any prior) the sync baseline for
    /// `(repo, number)`.
    fn set_issue_snapshot(
        &self,
        repo: &RepoId,
        number: u64,
        snapshot: IssueSnapshot,
    ) -> Result<(), ConfigError>;
}

/// A persisted sync baseline keyed by its forge issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotEntry {
    repo: RepoId,
    number: u64,
    snapshot: IssueSnapshot,
}

/// In-memory store. Cheap to `Clone`; all state is `Arc`'d.
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<MemoryInner>>,
}

#[derive(Default)]
struct MemoryInner {
    repo_bindings: Vec<RepoBinding>,
    issue_links: Vec<IssueLink>,
    snapshots: Vec<SnapshotEntry>,
    /// Indices kept as `HashMap` so reads don't scan vectors.
    /// Tiny crate's invariant: the indices are rebuilt
    /// alongside every write.
    by_project: HashMap<String, Vec<usize>>,
    by_task: HashMap<String, Vec<usize>>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl BindingStore for MemoryStore {
    fn add_repo_binding(&self, binding: RepoBinding) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        let idx = inner.repo_bindings.len();
        inner
            .by_project
            .entry(binding.project_id.clone())
            .or_default()
            .push(idx);
        inner.repo_bindings.push(binding);
        Ok(())
    }

    fn remove_repo_binding(&self, project_id: &str, repo: &RepoId) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .repo_bindings
            .retain(|b| !(b.project_id == project_id && &b.repo == repo));
        // Reindex — vectors are small enough that rebuilding
        // from scratch is fine until we move to SQLite.
        let MemoryInner {
            repo_bindings,
            by_project,
            ..
        } = &mut *inner;
        by_project.clear();
        for (i, b) in repo_bindings.iter().enumerate() {
            by_project.entry(b.project_id.clone()).or_default().push(i);
        }
        Ok(())
    }

    fn repos_for_project(&self, project_id: &str) -> Result<Vec<RepoId>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .by_project
            .get(project_id)
            .map(|idxs| {
                idxs.iter()
                    .map(|i| inner.repo_bindings[*i].repo.clone())
                    .collect()
            })
            .unwrap_or_default())
    }

    fn projects_for_repo(&self, repo: &RepoId) -> Result<Vec<String>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .repo_bindings
            .iter()
            .filter(|b| &b.repo == repo)
            .map(|b| b.project_id.clone())
            .collect())
    }

    fn add_issue_link(&self, link: IssueLink) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        let idx = inner.issue_links.len();
        inner
            .by_task
            .entry(link.task_id.clone())
            .or_default()
            .push(idx);
        inner.issue_links.push(link);
        Ok(())
    }

    fn remove_issue_link(&self, task_id: &str, number: u64) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .issue_links
            .retain(|l| !(l.task_id == task_id && l.number == number));
        let MemoryInner {
            issue_links,
            by_task,
            ..
        } = &mut *inner;
        by_task.clear();
        for (i, l) in issue_links.iter().enumerate() {
            by_task.entry(l.task_id.clone()).or_default().push(i);
        }
        Ok(())
    }

    fn issues_for_task(&self, task_id: &str) -> Result<Vec<IssueLink>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .by_task
            .get(task_id)
            .map(|idxs| idxs.iter().map(|i| inner.issue_links[*i].clone()).collect())
            .unwrap_or_default())
    }

    fn tasks_for_issue(&self, repo: &RepoId, number: u64) -> Result<Vec<String>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .issue_links
            .iter()
            .filter(|l| &l.repo == repo && l.number == number)
            .map(|l| l.task_id.clone())
            .collect())
    }

    fn get_issue_snapshot(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<Option<IssueSnapshot>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .snapshots
            .iter()
            .find(|s| &s.repo == repo && s.number == number)
            .map(|s| s.snapshot.clone()))
    }

    fn set_issue_snapshot(
        &self,
        repo: &RepoId,
        number: u64,
        snapshot: IssueSnapshot,
    ) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(e) = inner
            .snapshots
            .iter_mut()
            .find(|s| &s.repo == repo && s.number == number)
        {
            e.snapshot = snapshot;
        } else {
            inner.snapshots.push(SnapshotEntry {
                repo: repo.clone(),
                number,
                snapshot,
            });
        }
        Ok(())
    }
}

// ---- File-backed store -----------------------------------------------

/// JSON-backed `BindingStore` for the CLI / single-process server.
/// State persists across restarts at the configured path.
///
/// File layout — one `.json` per store, encoded shape:
///
/// ```json
/// {
///   "repo_bindings": [ { ... }, ... ],
///   "issue_links":   [ { ... }, ... ]
/// }
/// ```
///
/// Atomic writes: render to a sibling temp file, `fsync`, rename
/// into place. Cheap for the tens-to-hundreds-of-rows scale this
/// store is sized for. Swap to `SQLite` when the row count or
/// concurrent-writer surface justifies it (tracked as a roadmap
/// issue).
#[derive(Clone)]
pub struct FileStore {
    path: std::path::PathBuf,
    inner: Arc<Mutex<FileInner>>,
}

#[derive(Default, Serialize, Deserialize)]
struct FileInner {
    #[serde(default)]
    repo_bindings: Vec<RepoBinding>,
    #[serde(default)]
    issue_links: Vec<IssueLink>,
    #[serde(default)]
    snapshots: Vec<SnapshotEntry>,
}

impl FileStore {
    /// Open (or create) a store at `path`. Missing file → empty
    /// store. Existing file is parsed; corrupt JSON returns an
    /// error rather than silently dropping state.
    pub fn open(path: impl Into<std::path::PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        let inner = if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| ConfigError::Storage(e.to_string()))?;
            serde_json::from_slice::<FileInner>(&bytes)
                .map_err(|e| ConfigError::Storage(format!("parse {}: {e}", path.display())))?
        } else {
            FileInner::default()
        };
        Ok(Self {
            path,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Find the task linked to a forge issue identified by
    /// `(owner, repo, number)`, ignoring the forge host /
    /// base-url. Useful for the webhook receiver, which gets a
    /// `repository.full_name` but not the exact base-url the
    /// link was stored under. Returns the first match's
    /// `task_id` (links are unique per `(repo, number, kind)`).
    pub fn find_task_by_owner_repo_number(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Option<String>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .issue_links
            .iter()
            .find(|l| l.repo.owner == owner && l.repo.repo == repo && l.number == number)
            .map(|l| l.task_id.clone()))
    }

    /// Distinct repos that appear in the issue-link store —
    /// every forge repo this org has at least one linked issue
    /// on. Used by `task issue sync-all` to know what to reconcile.
    pub fn distinct_repos(&self) -> Result<Vec<RepoId>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<RepoId> = Vec::new();
        for l in &inner.issue_links {
            if !out.contains(&l.repo) {
                out.push(l.repo.clone());
            }
        }
        // Also include repos that only have a binding (no issues yet).
        for b in &inner.repo_bindings {
            if !out.contains(&b.repo) {
                out.push(b.repo.clone());
            }
        }
        Ok(out)
    }

    /// Distinct repos that are *connected* — i.e. bound to a project
    /// via a [`RepoBinding`]. Unlike [`Self::distinct_repos`] this
    /// ignores issue-link history, so it reflects the repos this org
    /// has deliberately wired up (what the `/repos` "connected" view
    /// shows), not every repo a stray issue once linked to.
    pub fn binding_repos(&self) -> Result<Vec<RepoId>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<RepoId> = Vec::new();
        for b in &inner.repo_bindings {
            if !out.contains(&b.repo) {
                out.push(b.repo.clone());
            }
        }
        Ok(out)
    }

    fn flush_locked(&self, inner: &FileInner) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Storage(e.to_string()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(inner)
            .map_err(|e| ConfigError::Storage(format!("encode: {e}")))?;
        std::fs::write(&tmp, json).map_err(|e| ConfigError::Storage(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| ConfigError::Storage(e.to_string()))?;
        Ok(())
    }
}

impl BindingStore for FileStore {
    fn add_repo_binding(&self, binding: RepoBinding) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        // De-dup: same (project_id, repo) tuple already present?
        // Replacing is a no-op; we don't carry extra metadata.
        if !inner
            .repo_bindings
            .iter()
            .any(|b| b.project_id == binding.project_id && b.repo == binding.repo)
        {
            inner.repo_bindings.push(binding);
            self.flush_locked(&inner)?;
        }
        Ok(())
    }

    fn remove_repo_binding(&self, project_id: &str, repo: &RepoId) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.repo_bindings.len();
        inner
            .repo_bindings
            .retain(|b| !(b.project_id == project_id && &b.repo == repo));
        if inner.repo_bindings.len() != before {
            self.flush_locked(&inner)?;
        }
        Ok(())
    }

    fn repos_for_project(&self, project_id: &str) -> Result<Vec<RepoId>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .repo_bindings
            .iter()
            .filter(|b| b.project_id == project_id)
            .map(|b| b.repo.clone())
            .collect())
    }

    fn projects_for_repo(&self, repo: &RepoId) -> Result<Vec<String>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .repo_bindings
            .iter()
            .filter(|b| &b.repo == repo)
            .map(|b| b.project_id.clone())
            .collect())
    }

    fn add_issue_link(&self, link: IssueLink) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        // Same de-dup rule: (task_id, repo, number, kind) tuple
        // is unique. Re-adding is a no-op.
        if !inner.issue_links.iter().any(|l| {
            l.task_id == link.task_id
                && l.repo == link.repo
                && l.number == link.number
                && l.kind == link.kind
        }) {
            inner.issue_links.push(link);
            self.flush_locked(&inner)?;
        }
        Ok(())
    }

    fn remove_issue_link(&self, task_id: &str, number: u64) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.issue_links.len();
        inner
            .issue_links
            .retain(|l| !(l.task_id == task_id && l.number == number));
        if inner.issue_links.len() != before {
            self.flush_locked(&inner)?;
        }
        Ok(())
    }

    fn issues_for_task(&self, task_id: &str) -> Result<Vec<IssueLink>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .issue_links
            .iter()
            .filter(|l| l.task_id == task_id)
            .cloned()
            .collect())
    }

    fn tasks_for_issue(&self, repo: &RepoId, number: u64) -> Result<Vec<String>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .issue_links
            .iter()
            .filter(|l| &l.repo == repo && l.number == number)
            .map(|l| l.task_id.clone())
            .collect())
    }

    fn get_issue_snapshot(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<Option<IssueSnapshot>, ConfigError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .snapshots
            .iter()
            .find(|s| &s.repo == repo && s.number == number)
            .map(|s| s.snapshot.clone()))
    }

    fn set_issue_snapshot(
        &self,
        repo: &RepoId,
        number: u64,
        snapshot: IssueSnapshot,
    ) -> Result<(), ConfigError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(e) = inner
            .snapshots
            .iter_mut()
            .find(|s| &s.repo == repo && s.number == number)
        {
            e.snapshot = snapshot;
        } else {
            inner.snapshots.push(SnapshotEntry {
                repo: repo.clone(),
                number,
                snapshot,
            });
        }
        self.flush_locked(&inner)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_proto::Forge;

    fn repo(host: &str, owner: &str, repo: &str) -> RepoId {
        RepoId {
            forge: Forge::Forgejo {
                base_url: format!("https://{host}"),
            },
            owner: owner.to_string(),
            repo: repo.to_string(),
        }
    }

    #[test]
    fn file_store_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("links.json");
        let r = repo("git.example.org", "acme", "widgets");

        // Write some state.
        {
            let s = FileStore::open(&path).unwrap();
            s.add_repo_binding(RepoBinding {
                project_id: "proj-1".into(),
                repo: r.clone(),
            })
            .unwrap();
            s.add_issue_link(IssueLink {
                task_id: "task-42".into(),
                repo: r.clone(),
                number: 7,
                kind: LinkKind::Issue,
            })
            .unwrap();
            // Same link twice — should de-dup.
            s.add_issue_link(IssueLink {
                task_id: "task-42".into(),
                repo: r.clone(),
                number: 7,
                kind: LinkKind::Issue,
            })
            .unwrap();
        }

        // Re-open and read.
        let s = FileStore::open(&path).unwrap();
        assert_eq!(s.repos_for_project("proj-1").unwrap(), vec![r.clone()]);
        let links = s.issues_for_task("task-42").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].number, 7);
        assert_eq!(s.tasks_for_issue(&r, 7).unwrap(), vec!["task-42"]);
    }

    #[test]
    fn snapshot_round_trips_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("links.json");
        let r = repo("git.example.org", "acme", "widgets");
        let ts = chrono::DateTime::parse_from_rfc3339("2026-05-29T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let snap = |t: &str| IssueSnapshot {
            task: SyncedFields {
                title: t.into(),
                body: "b".into(),
                closed: false,
            },
            forge: SyncedFields {
                title: t.into(),
                body: "b".into(),
                closed: false,
            },
            forge_updated_at: Some(ts),
        };

        {
            let s = FileStore::open(&path).unwrap();
            assert!(s.get_issue_snapshot(&r, 7).unwrap().is_none());
            s.set_issue_snapshot(&r, 7, snap("first")).unwrap();
            // Overwrite the same key.
            s.set_issue_snapshot(&r, 7, snap("second")).unwrap();
        }

        let s = FileStore::open(&path).unwrap();
        let got = s.get_issue_snapshot(&r, 7).unwrap().unwrap();
        assert_eq!(got.forge_updated_at, Some(ts));
        assert_eq!(got.task.title, "second");
        // Distinct key is independent.
        assert!(s.get_issue_snapshot(&r, 8).unwrap().is_none());
    }
}
