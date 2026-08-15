//! Server-side [`WorkstreamService`] backend. Walks the vault
//! on each call; resolves the owning project's slug when `path`
//! defaults need filling; publishes a [`WorkstreamEvent`] on
//! every successful mutation (the `#[subscribe]` stream).

use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;
use vault::Vault;

use crate::model::{Status, Workstream};
use crate::parse::{looks_like_workstream, parse_page};
use crate::rollup::rollup_with;
use crate::service::{WorkstreamError, WorkstreamEvent, WorkstreamService, WorkstreamWithRollup};
use crate::write::{default_workstream_path, write_workstream};

#[derive(Clone, architect::HasDispatcher)]
pub struct WorkstreamBackend {
    vault_root: PathBuf,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state
    /// here. Sliding mailbox: a slow subscriber loses its
    /// *oldest* queued events, which is correct for state-shaped
    /// payloads. Clones share the hub (Arc inside), so the
    /// service mount and the stream mount can each hold a
    /// backend clone. Mirrors `task::TaskBackend`.
    #[cfg(feature = "vox")]
    events: architect::PubSub<WorkstreamEvent>,
}

// Manual impl: `PubSub` carries no `Debug`.
impl std::fmt::Debug for WorkstreamBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkstreamBackend")
            .field("vault_root", &self.vault_root)
            .finish_non_exhaustive()
    }
}

impl WorkstreamBackend {
    #[must_use]
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: vault_root.into(),
            #[cfg(feature = "vox")]
            events: architect::PubSub::sliding(256),
        }
    }

    #[must_use]
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    /// Publish a workstream change to every `events` subscriber.
    /// Call only after the write succeeded — subscribers fold
    /// these into state fetched via `list()`, so a phantom event
    /// would desync them. No-op without the `vox` feature.
    fn publish(&self, event: WorkstreamEvent) {
        #[cfg(feature = "vox")]
        self.events.publish(event);
        #[cfg(not(feature = "vox"))]
        let _ = event;
    }

    fn open_vault(&self) -> Result<Vault, WorkstreamError> {
        Vault::open(&self.vault_root).map_err(|e| {
            WorkstreamError::Io(format!("open vault {}: {e}", self.vault_root.display()))
        })
    }

    fn list_inner(&self) -> Result<Vec<Workstream>, WorkstreamError> {
        let vault = self.open_vault()?;
        let mut out = Vec::new();
        for page in &vault.pages {
            let proto = page.to_proto();
            if !looks_like_workstream(&proto) {
                continue;
            }
            match parse_page(&proto) {
                Ok(w) => out.push(w),
                Err(e) => tracing::warn!(path = %page.rel_path, ?e, "workstream parse failed"),
            }
        }
        Ok(out)
    }

    fn find(&self, id: Uuid) -> Result<Workstream, WorkstreamError> {
        self.list_inner()?
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| WorkstreamError::NotFound(id.to_string()))
    }

    /// Every `type: task` page in the vault — the rollup input.
    fn org_tasks(&self) -> Result<Vec<task::TaskInfo>, WorkstreamError> {
        let vault = self.open_vault()?;
        let mut out = Vec::new();
        for page in &vault.pages {
            let proto = page.to_proto();
            if !task::parse::looks_like_task(&proto) {
                continue;
            }
            match task::parse_page(&proto) {
                Ok(t) => out.push(t),
                Err(e) => tracing::warn!(path = %page.rel_path, ?e, "task parse failed"),
            }
        }
        Ok(out)
    }

    /// Per-project state registries — project id → its optional
    /// `states:` config. Feeds the rollup's group classifier so
    /// custom status names classify by their registered group.
    fn project_states(
        &self,
    ) -> Result<std::collections::HashMap<Uuid, Option<project::StatesConfig>>, WorkstreamError>
    {
        let vault = self.open_vault()?;
        let mut out = std::collections::HashMap::new();
        for page in &vault.pages {
            let proto = page.to_proto();
            if !project::looks_like_project(&proto) {
                continue;
            }
            match project::parse_page(&proto) {
                Ok(p) => {
                    out.insert(p.id, p.states);
                }
                Err(e) => tracing::warn!(path = %page.rel_path, ?e, "project parse failed"),
            }
        }
        Ok(out)
    }

    /// Resolve the owning project's vault-relative path (e.g.
    /// `Projects/Health/Health.md`). Used by `create` to derive
    /// the default workstream path. Walks the vault for the one
    /// `type: project` page whose frontmatter `id` matches.
    fn project_path(&self, project_id: Uuid) -> Result<String, WorkstreamError> {
        let vault = self.open_vault()?;
        for page in &vault.pages {
            let Some((map, _)) = vault_entity::frontmatter::mapping(&page.raw) else {
                continue;
            };
            if vault_entity::yaml::str_at(&map, "type").as_deref() != Some("project") {
                continue;
            }
            let id_match = vault_entity::yaml::str_at(&map, "id")
                .and_then(|s| Uuid::parse_str(&s).ok())
                == Some(project_id);
            if id_match {
                return Ok(page.rel_path.clone());
            }
        }
        Err(WorkstreamError::BadRequest(format!(
            "no project with id {project_id} in this vault"
        )))
    }
}

impl WorkstreamService for WorkstreamBackend {
    fn list(&self, project: Option<Uuid>) -> Result<Vec<Workstream>, WorkstreamError> {
        Ok(self
            .list_inner()?
            .into_iter()
            .filter(|w| project.is_none_or(|pid| w.project_id == pid))
            .collect())
    }

    fn get(&self, id: Uuid) -> Result<Workstream, WorkstreamError> {
        self.find(id)
    }

    fn get_by_path(&self, path: &str) -> Result<Workstream, WorkstreamError> {
        self.list_inner()?
            .into_iter()
            .find(|w| w.path == path)
            .ok_or_else(|| WorkstreamError::NotFound(path.to_owned()))
    }

    fn create(&self, mut w: Workstream) -> Result<Workstream, WorkstreamError> {
        if w.title.trim().is_empty() {
            return Err(WorkstreamError::BadRequest("title is required".into()));
        }
        if w.project_id.is_nil() {
            return Err(WorkstreamError::BadRequest("project_id is required".into()));
        }
        if w.id.is_nil() {
            w.id = Uuid::new_v4();
        }
        if w.status.trim().is_empty() {
            w.status = Status::Backlog.as_str().into();
        }
        if w.path.is_empty() {
            let project_rel = self.project_path(w.project_id)?;
            w.path = default_workstream_path(&project_rel, &w.title);
        }
        let abs = self.vault_root.join(&w.path);
        if abs.exists() {
            return Err(WorkstreamError::AlreadyExists(w.path.clone()));
        }
        write_workstream(&self.vault_root, &mut w, false)
            .map_err(|e| WorkstreamError::Io(format!("write: {e}")))?;
        self.publish(WorkstreamEvent::Upserted(w.clone()));
        Ok(w)
    }

    fn update(&self, workstream: Workstream) -> Result<Workstream, WorkstreamError> {
        let existing = self.find(workstream.id)?;
        let mut next = workstream;
        next.path = existing.path;
        next.date_created = existing.date_created.or(next.date_created);
        next.date_modified = Some(Utc::now());
        write_workstream(&self.vault_root, &mut next, true)
            .map_err(|e| WorkstreamError::Io(format!("write: {e}")))?;
        self.publish(WorkstreamEvent::Upserted(next.clone()));
        Ok(next)
    }

    fn set_status(&self, id: Uuid, status: &str) -> Result<Workstream, WorkstreamError> {
        let canonical = Status::from_str(status).ok_or_else(|| {
            WorkstreamError::BadRequest(format!(
                "unknown status `{status}` — one of backlog / planned / in-progress / \
                 paused / done / cancelled"
            ))
        })?;
        let mut w = self.find(id)?;
        w.status = canonical.as_str().into();
        self.update(w)
    }

    fn delete(&self, id: Uuid) -> Result<(), WorkstreamError> {
        let w = self.find(id)?;
        // Refuse while tasks still point here — silently orphaning
        // `workflow.workstream` links would skew every rollup.
        let linked = self
            .org_tasks()?
            .into_iter()
            .find(|t| t.workflow.as_ref().and_then(|x| x.workstream) == Some(id));
        if let Some(t) = linked {
            return Err(WorkstreamError::BadRequest(format!(
                "{} is referenced by task {}; detach it first",
                w.title, t.path
            )));
        }
        let abs = self.vault_root.join(&w.path);
        std::fs::remove_file(&abs)
            .map_err(|e| WorkstreamError::Io(format!("remove {}: {e}", abs.display())))?;
        self.publish(WorkstreamEvent::Deleted(id));
        Ok(())
    }

    fn rollup(&self, id: Uuid) -> Result<WorkstreamWithRollup, WorkstreamError> {
        let workstream = self.find(id)?;
        let tasks = self.org_tasks()?;
        // Classify via each task's owning project's state
        // registry (default registry when the project declares
        // none / the task has no project).
        let states = self.project_states()?;
        let group_of = |t: &task::TaskInfo| {
            let cfg = t
                .project_id
                .and_then(|pid| states.get(&pid))
                .and_then(Option::as_ref);
            project::resolve_state_group(cfg, &t.status)
        };
        Ok(WorkstreamWithRollup {
            rollup: rollup_with(id, &tasks, group_of),
            workstream,
        })
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream
/// host the hub it attaches subscriber sinks to. Publishing
/// happens in the `WorkstreamService` impl above, on every
/// successful mutation.
#[cfg(feature = "vox")]
impl crate::service::WorkstreamServiceStreamSource for WorkstreamBackend {
    fn events_hub(&self) -> &architect::PubSub<WorkstreamEvent> {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentRefList, Links};

    fn seed_project(root: &Path) -> Uuid {
        let pid = Uuid::new_v4();
        let dir = root.join("Projects/Demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Demo.md"),
            format!("---\ntype: project\nid: {pid}\ntitle: Demo\n---\n"),
        )
        .unwrap();
        pid
    }

    fn new_ws(pid: Uuid, title: &str) -> Workstream {
        Workstream {
            path: String::new(),
            id: Uuid::nil(),
            title: title.into(),
            project_id: pid,
            status: String::new(),
            lead: None,
            members: AgentRefList::default(),
            start_date: None,
            target_date: None,
            links: Links::default(),
            date_created: None,
            date_modified: None,
            details: String::new(),
        }
    }

    #[test]
    fn create_derives_path_and_listing_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = seed_project(tmp.path());
        let be = WorkstreamBackend::new(tmp.path());

        let created = be.create(new_ws(pid, "ACP Adapter")).unwrap();
        assert_eq!(created.path, "Projects/Demo/workstreams/acp-adapter.md");
        assert_eq!(created.status, "backlog");
        assert!(!created.id.is_nil());

        let listed = be.list(Some(pid)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
        assert!(be.list(Some(Uuid::new_v4())).unwrap().is_empty());
    }

    #[test]
    fn set_status_canonicalizes_and_rejects_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = seed_project(tmp.path());
        let be = WorkstreamBackend::new(tmp.path());
        let ws = be.create(new_ws(pid, "Push")).unwrap();

        let active = be.set_status(ws.id, "active").unwrap();
        assert_eq!(active.status, "in-progress");
        assert!(matches!(
            be.set_status(ws.id, "warp-speed"),
            Err(WorkstreamError::BadRequest(_))
        ));
    }

    #[test]
    fn delete_refuses_while_tasks_attached() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = seed_project(tmp.path());
        let be = WorkstreamBackend::new(tmp.path());
        let ws = be.create(new_ws(pid, "Push")).unwrap();

        std::fs::create_dir_all(tmp.path().join("Task")).unwrap();
        std::fs::write(
            tmp.path().join("Task/sub.md"),
            format!(
                "---\ntype: task\nid: {}\ntitle: sub\nstatus: open\nworkflow:\n  workstream: {}\n---\n",
                Uuid::new_v4(),
                ws.id
            ),
        )
        .unwrap();

        assert!(matches!(
            be.delete(ws.id),
            Err(WorkstreamError::BadRequest(_))
        ));
        // Detach → delete succeeds.
        std::fs::remove_file(tmp.path().join("Task/sub.md")).unwrap();
        be.delete(ws.id).unwrap();
        assert!(be.list(None).unwrap().is_empty());
    }

    #[test]
    fn rollup_over_vault_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = seed_project(tmp.path());
        let be = WorkstreamBackend::new(tmp.path());
        let ws = be.create(new_ws(pid, "Push")).unwrap();

        let task_dir = tmp.path().join("Task");
        std::fs::create_dir_all(&task_dir).unwrap();
        for (i, (status, size)) in [("done", "s"), ("in-progress", "m"), ("open", "l")]
            .iter()
            .enumerate()
        {
            std::fs::write(
                task_dir.join(format!("t{i}.md")),
                format!(
                    "---\ntype: task\nid: {}\ntitle: t{i}\nstatus: {status}\nworkflow:\n  workstream: {}\n  estimate:\n    size: {size}\n---\n",
                    Uuid::new_v4(),
                    ws.id
                ),
            )
            .unwrap();
        }

        let got = be.rollup(ws.id).unwrap();
        assert_eq!(got.workstream.id, ws.id);
        assert_eq!(got.rollup.total, 3);
        assert_eq!(got.rollup.done, 1);
        assert_eq!(got.rollup.in_progress, 1);
        assert_eq!(got.rollup.blocked, 0);
        assert_eq!(got.rollup.estimate_points_sum, 2 + 3 + 5);
    }

    /// Custom status names classify through the owning
    /// project's `states:` registry — `shipped-to-client` is
    /// meaningless to the default registry but the project maps
    /// it to the completed group.
    #[test]
    fn rollup_classifies_via_project_state_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = Uuid::new_v4();
        let dir = tmp.path().join("Projects/Demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Demo.md"),
            format!(
                "---\ntype: project\nid: {pid}\ntitle: Demo\nstates:\n- name: shipped-to-client\n  group: completed\n- name: qa\n  group: started\n---\n"
            ),
        )
        .unwrap();
        let be = WorkstreamBackend::new(tmp.path());
        let ws = be.create(new_ws(pid, "Push")).unwrap();

        let task_dir = tmp.path().join("Task");
        std::fs::create_dir_all(&task_dir).unwrap();
        for (i, status) in ["shipped-to-client", "qa", "open"].iter().enumerate() {
            std::fs::write(
                task_dir.join(format!("t{i}.md")),
                format!(
                    "---\ntype: task\nid: {}\ntitle: t{i}\nstatus: {status}\nprojectId: {pid}\nworkflow:\n  workstream: {}\n---\n",
                    Uuid::new_v4(),
                    ws.id
                ),
            )
            .unwrap();
        }

        let got = be.rollup(ws.id).unwrap();
        assert_eq!(got.rollup.total, 3);
        assert_eq!(got.rollup.done, 1, "shipped-to-client → completed group");
        assert_eq!(got.rollup.in_progress, 1, "qa → started group");
        // Per-group counts ride the same registry resolution.
        assert_eq!(got.rollup.groups.completed, 1);
        assert_eq!(got.rollup.groups.started, 1);
        assert_eq!(got.rollup.groups.unstarted, 1);
        assert_eq!(got.rollup.groups.backlog, 0);
        assert_eq!(got.rollup.groups.cancelled, 0);
    }
}
