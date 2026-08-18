//! Server-side [`ProjectService`] backend.
//!
//! Walks the configured vault root on each call (cheap — the
//! vault page index lives in memory once `Vault::open` runs).
//! `ProjectBackend` is what the task-server mounts under
//! `/org/<slug>/vox`; the architect-rpc macro emits a sync
//! shim, so the server-bridge can call this directly even
//! though the trait surface is sync.
//!
//! Cheap to `Clone` — the inner [`std::path::PathBuf`] is
//! reused; each request re-opens the vault. Future
//! optimization: cache the parsed list with an mtime check.

use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;
use vault::Vault;

use crate::model::ProjectInfo;
use crate::parts::Part;
use crate::scan::scan_vault;
use crate::service::{ProjectError, ProjectService};
use crate::write::{default_project_path, write_project};

/// File-backed `ProjectService` impl. Built once at server
/// boot per org, cloned into the vox bridge.
#[derive(Clone, architect::HasDispatcher)]
pub struct ProjectBackend {
    vault_root: PathBuf,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state
    /// here ([`ProjectEvent::Upserted`] / [`ProjectEvent::Deleted`]).
    /// Sliding mailbox: a slow subscriber loses its *oldest* queued
    /// events, which is correct for state-shaped payloads. Clones
    /// share the hub (it's `Arc` inside), so the service mount and
    /// the stream mount can each hold a backend clone.
    #[cfg(feature = "vox")]
    events: architect::PubSub<crate::service::ProjectEvent>,
}

// Manual impl: `PubSub` carries no `Debug`.
impl std::fmt::Debug for ProjectBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectBackend")
            .field("vault_root", &self.vault_root)
            .finish_non_exhaustive()
    }
}

impl ProjectBackend {
    #[must_use]
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: vault_root.into(),
            #[cfg(feature = "vox")]
            events: architect::PubSub::sliding(256),
        }
    }

    /// Publish a project change to every `events` subscriber. Call
    /// only after the write succeeded — subscribers fold these into
    /// state fetched via `list()`, so a phantom event would desync
    /// them. No-op without the `vox` feature (no wire, no
    /// subscribers).
    fn publish(&self, event: crate::service::ProjectEvent) {
        #[cfg(feature = "vox")]
        self.events.publish(event);
        #[cfg(not(feature = "vox"))]
        let _ = event;
    }

    /// Vault root this backend reads from.
    #[must_use]
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    /// Write a project back to its page and announce it.
    ///
    /// The part verbs all have the same shape — read the page, change
    /// one list, save — and routing them through `update` would carry
    /// the caller's whole `ProjectInfo` when what changed is one field.
    fn save(&self, mut project: ProjectInfo) -> Result<ProjectInfo, ProjectError> {
        project.date_modified = Some(Utc::now());
        write_project(&self.vault_root, &mut project, true)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(project.clone()));
        Ok(project)
    }

    fn list_inner(&self) -> Result<Vec<ProjectInfo>, ProjectError> {
        let vault = Vault::open(&self.vault_root).map_err(|e| {
            ProjectError::Io(format!("open vault {}: {e}", self.vault_root.display()))
        })?;
        scan_vault(&vault).map_err(|e| ProjectError::Io(format!("scan: {e}")))
    }
}

impl ProjectService for ProjectBackend {
    fn list(&self) -> Result<Vec<ProjectInfo>, ProjectError> {
        self.list_inner()
    }

    fn get(&self, id: Uuid) -> Result<ProjectInfo, ProjectError> {
        self.list_inner()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ProjectError::NotFound(id.to_string()))
    }

    fn get_by_path(&self, path: &str) -> Result<ProjectInfo, ProjectError> {
        self.list_inner()?
            .into_iter()
            .find(|p| p.path == path)
            .ok_or_else(|| ProjectError::NotFound(path.to_owned()))
    }

    fn create(&self, mut project: ProjectInfo) -> Result<ProjectInfo, ProjectError> {
        if project.title.trim().is_empty() {
            return Err(ProjectError::BadRequest("title is required".into()));
        }
        if project.id.is_nil() {
            project.id = Uuid::new_v4();
        }
        if project.path.is_empty() {
            project.path = default_project_path(&project.title);
        }
        let now = Utc::now();
        if project.date_created.is_none() {
            project.date_created = Some(now);
        }
        project.date_modified = Some(now);

        let abs = self.vault_root.join(&project.path);
        if abs.exists() {
            return Err(ProjectError::AlreadyExists(project.path.clone()));
        }
        write_project(&self.vault_root, &mut project, false)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(project.clone()));
        Ok(project)
    }

    fn update(&self, project: ProjectInfo) -> Result<ProjectInfo, ProjectError> {
        let existing = self
            .list_inner()?
            .into_iter()
            .find(|p| p.id == project.id)
            .ok_or_else(|| ProjectError::NotFound(project.id.to_string()))?;
        // Carry the on-disk path forward — caller cannot
        // smuggle a rename through `update`.
        let mut next = project;
        next.path = existing.path;
        next.date_created = existing.date_created.or(next.date_created);
        next.date_modified = Some(Utc::now());
        write_project(&self.vault_root, &mut next, true)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(next.clone()));
        Ok(next)
    }

    fn rename(&self, id: Uuid, new_path: &str) -> Result<ProjectInfo, ProjectError> {
        if new_path.is_empty() || new_path.contains("..") || new_path.starts_with('/') {
            return Err(ProjectError::BadRequest(format!("bad path: {new_path}")));
        }
        let mut p = self
            .list_inner()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ProjectError::NotFound(id.to_string()))?;
        let from = self.vault_root.join(&p.path);
        let to = self.vault_root.join(new_path);
        if to.exists() {
            return Err(ProjectError::AlreadyExists(new_path.to_owned()));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ProjectError::Io(format!("mkdir: {e}")))?;
        }
        std::fs::rename(&from, &to).map_err(|e| ProjectError::Io(format!("rename: {e}")))?;
        p.path = new_path.to_owned();
        p.date_modified = Some(Utc::now());
        // Re-serialize so frontmatter mtime tracks the move.
        write_project(&self.vault_root, &mut p, true)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(p.clone()));
        Ok(p)
    }

    fn parts(&self, project: Uuid) -> Result<Vec<Part>, ProjectError> {
        Ok(self.get(project)?.parts.0)
    }

    fn add_part(&self, project: Uuid, name: &str) -> Result<Part, ProjectError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProjectError::BadRequest("a part needs a name".into()));
        }
        let mut p = self.get(project)?;
        if p.parts.has_name(name) {
            return Err(ProjectError::AlreadyExists(format!(
                "{} already has a part called {name}",
                p.title
            )));
        }
        // An id now, not at promotion time — see `project_proto::parts`.
        let part = Part {
            id: Uuid::new_v4(),
            name: name.to_owned(),
        };
        p.parts.0.push(part.clone());
        self.save(p)?;
        Ok(part)
    }

    fn rename_part(&self, project: Uuid, part: Uuid, name: &str) -> Result<Part, ProjectError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProjectError::BadRequest("a part needs a name".into()));
        }
        let mut p = self.get(project)?;
        // A rename onto a name another part holds is the same collision
        // `add_part` refuses; renaming a part to what it already is, is
        // not.
        if p.parts
            .0
            .iter()
            .any(|x| x.id != part && x.name.eq_ignore_ascii_case(name))
        {
            return Err(ProjectError::AlreadyExists(format!(
                "{} already has a part called {name}",
                p.title
            )));
        }
        let found = p
            .parts
            .0
            .iter_mut()
            .find(|x| x.id == part)
            .ok_or_else(|| ProjectError::NotFound(part.to_string()))?;
        found.name = name.to_owned();
        let renamed = found.clone();
        self.save(p)?;
        Ok(renamed)
    }

    fn remove_part(&self, project: Uuid, part: Uuid) -> Result<(), ProjectError> {
        let mut p = self.get(project)?;
        let before = p.parts.len();
        p.parts.0.retain(|x| x.id != part);
        if p.parts.len() == before {
            return Err(ProjectError::NotFound(part.to_string()));
        }
        self.save(p)?;
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), ProjectError> {
        let all = self.list_inner()?;
        let p = all
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ProjectError::NotFound(id.to_string()))?;
        // Refuse if any other project parents off this one —
        // orphans here would silently float to the root in the
        // UI and quietly break agent reasoning.
        if let Some(child) = all.iter().find(|c| c.parent_id == Some(id)) {
            return Err(ProjectError::BadRequest(format!(
                "{} is parent of {}; reparent or delete children first",
                p.title, child.title
            )));
        }
        let abs = self.vault_root.join(&p.path);
        std::fs::remove_file(&abs)
            .map_err(|e| ProjectError::Io(format!("remove {}: {e}", abs.display())))?;
        self.publish(crate::service::ProjectEvent::Deleted(id));
        Ok(())
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `ProjectService` impl above, on every successful mutation.
#[cfg(feature = "vox")]
impl crate::service::ProjectServiceStreamSource for ProjectBackend {
    fn events_hub(&self) -> &architect::PubSub<crate::service::ProjectEvent> {
        &self.events
    }
}
