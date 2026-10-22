//! Server-side [`TaskService`] backend — walks the configured
//! vault on each call.
//!
//! Mirror of `project::ProjectBackend` / `goal::GoalBackend`.
//! Cheap to `Clone`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use uuid::Uuid;
use vault::Vault;

use crate::model::TaskInfo;
use crate::parse::{looks_like_task_raw, parse_page};
use crate::service::{
    ClaimResult, TaskError, TaskEvent, TaskListFilter, TaskReverseRelations, TaskService,
};
use crate::write::{default_task_path, write_task};

/// What [`TaskBackend::notice_file`] / [`TaskBackend::notice_removed`]
/// did with a change — the `task_sync.outcome` field on its span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noticed {
    /// The file is a task; announced as [`TaskEvent::Upserted`].
    Upserted,
    /// The path held a task that is gone (removed, or no longer a task);
    /// announced as [`TaskEvent::Deleted`].
    Deleted,
    /// Not a task, and none was known there — nothing announced.
    NotATask,
}

impl Noticed {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upserted => "upserted",
            Self::Deleted => "deleted",
            Self::NotATask => "not_a_task",
        }
    }
}

#[derive(Clone, architect::HasDispatcher)]
pub struct TaskBackend {
    vault_root: PathBuf,
    /// Serializes `try_claim` read-check-write so two concurrent
    /// claims on the same task can't both win. One server process
    /// per org, so a process-local mutex is the whole story.
    claim_lock: Arc<Mutex<()>>,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state
    /// here ([`TaskEvent::Upserted`] / [`TaskEvent::Deleted`]).
    /// Sliding mailbox: a slow subscriber loses its *oldest*
    /// queued events, which is correct for state-shaped payloads.
    /// Clones share the hub (it's `Arc` inside), so the service
    /// mount and the stream mount can each hold a backend clone.
    #[cfg(feature = "vox")]
    events: architect::PubSub<TaskEvent>,
    /// Which task each file holds, as last seen — so a file removed
    /// from outside the service (a peer's sync, the desktop folder,
    /// Obsidian) can still be announced as [`TaskEvent::Deleted`] by id.
    paths: Arc<Mutex<std::collections::HashMap<String, Uuid>>>,
}

// Manual impl: `PubSub` carries no `Debug`.
impl std::fmt::Debug for TaskBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskBackend")
            .field("vault_root", &self.vault_root)
            .finish_non_exhaustive()
    }
}

impl TaskBackend {
    #[must_use]
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: vault_root.into(),
            claim_lock: Arc::new(Mutex::new(())),
            #[cfg(feature = "vox")]
            events: architect::PubSub::sliding(256),
            paths: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// A vault file changed by something other than this service — a
    /// document merged from a peer and written by the collab layer, an
    /// edit in the desktop folder, an agent writing markdown — with its
    /// current text. When it is a task, announce it as
    /// [`TaskEvent::Upserted`], so every list, the activity feed and the
    /// notifier see it exactly as they see a write made here. A file that
    /// stopped being a task (its `type` changed) is announced as gone.
    /// Writes this service made come back here too; announcing the same
    /// state twice is harmless, since subscribers fold state.
    // t[impl offline.doc.materialise]
    pub fn notice_file(&self, rel_path: &str, raw: &str) -> Noticed {
        match task_proto::doc::task_of(rel_path, raw) {
            Some(task) => {
                self.remember(rel_path, task.id);
                self.publish(TaskEvent::Upserted(task));
                Noticed::Upserted
            }
            None => self.notice_removed(rel_path),
        }
    }

    /// A vault file was removed by something other than this service.
    /// When it held a task, announce [`TaskEvent::Deleted`].
    pub fn notice_removed(&self, rel_path: &str) -> Noticed {
        let gone = self.paths.lock().ok().and_then(|mut m| m.remove(rel_path));
        match gone {
            Some(id) => {
                self.publish(TaskEvent::Deleted(id));
                Noticed::Deleted
            }
            None => Noticed::NotATask,
        }
    }

    fn remember(&self, rel_path: &str, id: Uuid) {
        if let Ok(mut m) = self.paths.lock() {
            m.retain(|_, known| *known != id);
            m.insert(rel_path.to_owned(), id);
        }
    }

    /// Publish a task change to every `events` subscriber. Call only
    /// after the write succeeded — subscribers fold these into state
    /// fetched via `list()`, so a phantom event would desync them.
    /// No-op without the `vox` feature (no wire, no subscribers).
    fn publish(&self, event: TaskEvent) {
        #[cfg(feature = "vox")]
        self.events.publish(event);
        #[cfg(not(feature = "vox"))]
        let _ = event;
    }

    #[must_use]
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    fn list_inner(&self) -> Result<Vec<TaskInfo>, TaskError> {
        let vault = Vault::open(&self.vault_root)
            .map_err(|e| TaskError::Io(format!("open vault {}: {e}", self.vault_root.display())))?;
        let mut out = Vec::new();
        // By value, and tested on the raw text first: a page that is not
        // a task is dropped without being copied, and one that is moves
        // into the proto shape instead of being cloned into it. This
        // runs over the whole vault on every list and every get.
        for page in vault.pages {
            if !looks_like_task_raw(&page.raw) {
                continue;
            }
            // The parser consumes the wasm-clean
            // `vault_proto::VaultPage`; convert the live page first.
            let proto = page.into_proto();
            match parse_page(&proto) {
                Ok(t) => {
                    self.remember(&t.path, t.id);
                    out.push(t);
                }
                Err(e) => tracing::warn!(path = %proto.rel_path, ?e, "task parse failed"),
            }
        }
        Ok(out)
    }
}

impl TaskService for TaskBackend {
    fn list(&self) -> Result<Vec<TaskInfo>, TaskError> {
        self.list_inner()
    }

    fn get(&self, id: Uuid) -> Result<TaskInfo, TaskError> {
        self.list_inner()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))
    }

    fn get_by_path(&self, path: &str) -> Result<TaskInfo, TaskError> {
        self.list_inner()?
            .into_iter()
            .find(|t| t.path == path)
            .ok_or_else(|| TaskError::NotFound(path.to_owned()))
    }

    fn create(&self, mut task: TaskInfo) -> Result<TaskInfo, TaskError> {
        if task.title.trim().is_empty() {
            return Err(TaskError::BadRequest("title is required".into()));
        }
        if task.id.is_nil() {
            task.id = Uuid::new_v4();
        }
        if task.path.is_empty() {
            // The file name comes from the title, and two tasks may
            // share a title ("Call the venue" twice a year). A name the
            // caller did not choose is never a reason to refuse: take
            // the first free `-2`, `-3`, … beside it.
            task.path = free_path(&self.vault_root, &default_task_path(&task.title, None));
        }
        let now = Utc::now();
        if task.date_created.is_none() {
            task.date_created = Some(now);
        }
        task.date_modified = Some(now);

        // A path the caller chose is theirs: never overwrite what is there.
        let abs = self.vault_root.join(&task.path);
        if abs.exists() {
            return Err(TaskError::AlreadyExists(task.path.clone()));
        }
        write_task(&self.vault_root, &mut task, false)
            .map_err(|e| TaskError::Io(format!("write: {e}")))?;
        self.publish(TaskEvent::Upserted(task.clone()));
        Ok(task)
    }

    fn update(&self, task: TaskInfo) -> Result<TaskInfo, TaskError> {
        let existing = self
            .list_inner()?
            .into_iter()
            .find(|t| t.id == task.id)
            .ok_or_else(|| TaskError::NotFound(task.id.to_string()))?;
        let mut next = task;
        next.path = existing.path;
        next.date_created = existing.date_created.or(next.date_created);
        // t[impl attribution.immutable]
        // Who created a task is write-once: an update keeps the creator
        // on disk, whatever the incoming task says (or leaves out).
        next.created_by = existing.created_by;
        next.created_by_name = existing.created_by_name;
        next.on_behalf_of = existing.on_behalf_of;
        next.on_behalf_of_name = existing.on_behalf_of_name;
        next.date_modified = Some(Utc::now());
        // Automatic time tracking on status transitions (authoritative
        // here so CLI / cascade / any client gets it): entering
        // in-progress starts an inline TimeEntry, leaving it closes the
        // open one. Compared against the on-disk status, so a client
        // that already mirrored the tracking optimistically is a no-op.
        crate::model::track_status_transition(&existing.status, &mut next, Utc::now());
        write_task(&self.vault_root, &mut next, true)
            .map_err(|e| TaskError::Io(format!("write: {e}")))?;
        self.publish(TaskEvent::Upserted(next.clone()));
        // Family cascade (see relations::cascade_status): completing a
        // parent completes its subtasks, the last-done subtask completes
        // its parent, a reopened subtask reopens a done parent. Applied
        // through this same method so each hop writes + publishes; the
        // rule only ever flips terminal-ness, so recursion converges.
        let all = self.list_inner()?;
        for (id, status) in crate::relations::cascade_status(&all, &next) {
            if let Some(mut follow) = all.iter().find(|t| t.id == id).cloned() {
                follow.status = status;
                // Best-effort: a failed cascade write must not fail the
                // primary update the caller asked for.
                let _ = self.update(follow);
            }
        }
        Ok(next)
    }

    fn try_claim(&self, id: Uuid, agent: String, force: bool) -> Result<ClaimResult, TaskError> {
        let agent: workflows_proto::AgentRef = serde_json::from_str(&agent)
            .map_err(|e| TaskError::BadRequest(format!("agent ref json: {e}")))?;
        // Hold the lock across read → check → write so no other
        // claim can interleave. Truly atomic within the process.
        let _guard = self
            .claim_lock
            .lock()
            .map_err(|_| TaskError::Io("claim lock poisoned".into()))?;
        let mut t = self
            .list_inner()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
        let w = t
            .workflow
            .get_or_insert_with(crate::model::WorkflowAttrs::default);
        if let Some(holder) = w.assignees.0.first() {
            if holder == &agent {
                return Ok(ClaimResult::AlreadyMine);
            }
            if !force {
                return Ok(ClaimResult::Lost {
                    holder: holder.short_label(),
                });
            }
        }
        w.assignees = crate::model::AgentRefList(vec![agent]);
        // Inline the write (we already hold the claim lock; calling
        // self.update would re-list but that's fine — keep it simple
        // and write directly).
        t.date_modified = Some(Utc::now());
        write_task(&self.vault_root, &mut t, true)
            .map_err(|e| TaskError::Io(format!("write: {e}")))?;
        // A won claim changed the assignees — that's an upsert. The
        // `AlreadyMine` / `Lost` early returns above write nothing,
        // so they (correctly) publish nothing.
        self.publish(TaskEvent::Upserted(t));
        Ok(ClaimResult::Won)
    }

    fn reverse_relations(
        &self,
        id: Uuid,
    ) -> Result<Vec<crate::relations::ReverseRelation>, TaskError> {
        // Build the reverse index over the same snapshot list()
        // serves. Per-call build matches the walk-on-each-call
        // architecture of this backend; promote to a cached
        // index if vault walks get hot.
        let tasks = self.list_inner()?;
        if !tasks.iter().any(|t| t.id == id) {
            return Err(TaskError::NotFound(id.to_string()));
        }
        Ok(crate::relations::reverse_relations_for(id, &tasks))
    }

    fn reverse_relations_batch(
        &self,
        ids: Vec<Uuid>,
    ) -> Result<Vec<TaskReverseRelations>, TaskError> {
        // One vault walk + one reverse index for the whole
        // batch — the point of the verb. Unknown ids come back
        // with empty `relations` (no per-id NotFound: a list row
        // asking about a just-deleted task shouldn't fail the
        // whole page).
        let tasks = self.list_inner()?;
        let mut idx = crate::relations::reverse_index(&tasks);
        Ok(ids
            .into_iter()
            .map(|id| TaskReverseRelations {
                id,
                relations: idx.remove(&id).unwrap_or_default(),
            })
            .collect())
    }

    fn query(&self, filter: TaskListFilter) -> Result<Vec<TaskInfo>, TaskError> {
        let mut rows: Vec<TaskInfo> = self
            .list_inner()?
            .into_iter()
            .filter(|t| filter.project.is_none_or(|pid| t.project_id == Some(pid)))
            .filter(|t| {
                filter
                    .workstream
                    .is_none_or(|ws| t.workflow.as_ref().and_then(|w| w.workstream) == Some(ws))
            })
            .filter(|t| {
                filter
                    .status
                    .as_deref()
                    .is_none_or(|s| t.status.eq_ignore_ascii_case(s))
            })
            .filter(|t| !filter.open_only || crate::status_is_open(&t.status))
            .filter(|t| {
                filter.due_on_or_before.as_deref().is_none_or(|date| {
                    crate::is_due_on_or_before(t.due.as_deref(), t.scheduled.as_deref(), date)
                })
            })
            .filter(|t| {
                // Unfiled work is triage, not "what should I do now"
                // — the same exclusion `filter_relevant` applies for
                // the UI, so `--relevant` means one thing everywhere.
                filter.relevance.as_ref().is_none_or(|ctx| {
                    task_proto::is_filed(t) && crate::relevance::is_relevant(t, ctx)
                })
            })
            .filter(|t| {
                filter.created_by.as_deref().is_none_or(|who| {
                    workflows_proto::creator::creator_matches(t.created_by.as_deref(), who)
                })
            })
            .filter(|t| {
                filter.on_behalf_of.as_deref().is_none_or(|who| {
                    workflows_proto::creator::creator_matches(t.on_behalf_of.as_deref(), who)
                })
            })
            .filter(|t| {
                filter.assignee.as_deref().is_none_or(|who| {
                    let assignees = t.workflow.as_ref().map_or(&[][..], |w| &w.assignees.0[..]);
                    workflows_proto::creator::assignee_matches(assignees, who)
                })
            })
            .collect();
        // Stable page windows: order by vault-relative path
        // (unique per task) before slicing, so offset/limit
        // pages don't shear between calls. Relevance rank (when
        // asked for) sorts on top of that stable order.
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        if let Some(ctx) = &filter.relevance {
            // Relevant = the condensed "right now" view: one next
            // action per project, ranked.
            crate::relevance::condense_next_per_project(&mut rows);
            rows.sort_by_key(|t| crate::relevance::relevance_rank(t, ctx));
        }
        let offset = filter.offset.unwrap_or(0) as usize;
        let rows = rows.into_iter().skip(offset);
        Ok(match filter.limit {
            Some(n) => rows.take(n as usize).collect(),
            None => rows.collect(),
        })
    }

    fn rename(&self, id: Uuid, new_path: &str) -> Result<TaskInfo, TaskError> {
        if new_path.is_empty() || new_path.contains("..") || new_path.starts_with('/') {
            return Err(TaskError::BadRequest(format!("bad path: {new_path}")));
        }
        let mut t = self
            .list_inner()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
        if self.vault_root.join(new_path).exists() {
            return Err(TaskError::AlreadyExists(new_path.to_owned()));
        }
        // Through the vault's write path, like every other page mutation
        // here (`project.vault.write-path`).
        vault::move_page_at(&self.vault_root, &t.path, new_path)
            .map_err(|e| TaskError::Io(format!("rename: {e}")))?;
        t.path = new_path.to_owned();
        t.date_modified = Some(Utc::now());
        write_task(&self.vault_root, &mut t, true)
            .map_err(|e| TaskError::Io(format!("write: {e}")))?;
        self.publish(TaskEvent::Upserted(t.clone()));
        Ok(t)
    }

    fn delete(&self, id: Uuid) -> Result<(), TaskError> {
        let t = self
            .list_inner()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
        vault::delete_page_at(&self.vault_root, &t.path)
            .map_err(|e| TaskError::Io(format!("remove {}: {e}", t.path)))?;
        self.publish(TaskEvent::Deleted(id));
        Ok(())
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `TaskService` impl above, on every successful mutation.
#[cfg(feature = "vox")]
impl crate::service::TaskServiceStreamSource for TaskBackend {
    fn events_hub(&self) -> &architect::PubSub<TaskEvent> {
        &self.events
    }
}

/// `path` if nothing is there, else the first of `stem-2.md`,
/// `stem-3.md`, … that is free.
fn free_path(vault_root: &Path, path: &str) -> String {
    if !vault_root.join(path).exists() {
        return path.to_owned();
    }
    let stem = path.strip_suffix(".md").unwrap_or(path);
    (2..)
        .map(|n| format!("{stem}-{n}.md"))
        .find(|p| !vault_root.join(p).exists())
        .expect("an unbounded range always finds a free name")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Relation, RelationKind, RelationList, WorkflowAttrs};
    use crate::service::TaskListFilter;

    /// Two tasks may share a title; the second gets its own file
    /// rather than an `AlreadyExists` the person never caused.
    #[test]
    fn tasks_with_the_same_title_are_both_created() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());
        let first = be.create(crate::capture("Buy strings")).expect("first");
        let second = be.create(crate::capture("Buy strings")).expect("second");
        let third = be.create(crate::capture("Buy strings")).expect("third");
        assert_eq!(first.path, "Task/buy-strings.md");
        assert_eq!(second.path, "Task/buy-strings-2.md");
        assert_eq!(third.path, "Task/buy-strings-3.md");

        // A path the caller chose is still never overwritten.
        let mut chosen = crate::capture("Other");
        chosen.path = first.path.clone();
        assert!(matches!(
            be.create(chosen),
            Err(TaskError::AlreadyExists(_))
        ));
    }

    /// A task file written or removed by something other than the service
    /// (a peer's sync, the desktop folder) is announced like a write made
    /// here: Upserted with its state, then Deleted by the id it held.
    #[cfg(feature = "vox")]
    #[test]
    // t[verify offline.doc.materialise]
    fn a_task_file_changed_from_outside_is_announced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());
        let rx = be.events.subscribe(16);
        let mut t = crate::capture("Restring the bass @studio");
        t.path = "Task/restring-the-bass.md".into();
        let raw = task_proto::doc::serialize_task(&t).expect("serialize");

        be.notice_file(&t.path, &raw);
        match rx.try_recv().expect("an event") {
            TaskEvent::Upserted(got) => {
                assert_eq!(got.id, t.id);
                assert_eq!(got.title, t.title);
            }
            TaskEvent::Deleted(_) => panic!("expected the task"),
        }

        // A note is not a task, and nothing was known at its path.
        be.notice_file("Notes/idea.md", "---\ntitle: Idea\n---\nhi\n");
        assert!(rx.try_recv().is_err());

        be.notice_removed(&t.path);
        assert!(matches!(rx.try_recv(), Ok(TaskEvent::Deleted(id)) if id == t.id));
        // Once only.
        be.notice_removed(&t.path);
        assert!(rx.try_recv().is_err());
    }

    fn make(
        be: &TaskBackend,
        title: &str,
        status: &str,
        project: Option<Uuid>,
        workstream: Option<Uuid>,
    ) -> TaskInfo {
        let mut t = crate::capture(title);
        t.status = status.into();
        t.project_id = project;
        if let Some(ws) = workstream {
            t.workflow = Some(WorkflowAttrs {
                workstream: Some(ws),
                ..Default::default()
            });
        }
        be.create(t).expect("create")
    }

    #[test]
    fn status_transitions_track_time_and_cascade_stops_the_parent_clock() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());

        let parent = make(&be, "wind down", "open", None, None);
        let mut child = crate::capture("floss");
        child.workflow = Some(WorkflowAttrs {
            parent: Some(parent.id),
            ..Default::default()
        });
        let child = be.create(child).expect("create child");

        // Start the parent → an open TimeEntry appears.
        let mut p = be.get(parent.id).expect("get");
        p.status = "in-progress".into();
        be.update(p).expect("start parent");
        let p = be.get(parent.id).expect("get");
        assert_eq!(p.time_entries.0.len(), 1);
        assert!(p.time_entries.0[0].end_time.is_none());

        // Complete the only child → cascade completes the parent AND
        // closes its running entry ("when it's done the wind down
        // timer is done").
        let mut c = be.get(child.id).expect("get");
        c.status = "done".into();
        be.update(c).expect("finish child");
        let p = be.get(parent.id).expect("get");
        assert_eq!(p.status, "done");
        assert_eq!(p.time_entries.0.len(), 1);
        assert!(p.time_entries.0[0].end_time.is_some());
    }

    #[test]
    fn update_cascades_across_the_family() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());

        let parent = make(&be, "wind down", "open", None, None);
        let child = |title: &str| {
            let mut t = crate::capture(title);
            t.workflow = Some(WorkflowAttrs {
                parent: Some(parent.id),
                ..Default::default()
            });
            be.create(t).expect("create child")
        };
        let a = child("floss");
        let b = child("shower");

        let status_of = |id| be.get(id).expect("get").status;

        // First child done → parent stays open.
        let mut a_done = a.clone();
        a_done.status = "done".into();
        be.update(a_done).expect("update a");
        assert_eq!(status_of(parent.id), "open");

        // Last child done → parent completes.
        let mut b_done = b.clone();
        b_done.status = "done".into();
        be.update(b_done).expect("update b");
        assert_eq!(status_of(parent.id), "done");

        // Reopen a child → parent reopens (other child untouched).
        let mut a_open = be.get(a.id).expect("get a");
        a_open.status = "open".into();
        be.update(a_open).expect("reopen a");
        assert_eq!(status_of(parent.id), "open");
        assert_eq!(status_of(b.id), "done");

        // Complete the parent → the open child completes with it.
        let mut p_done = be.get(parent.id).expect("get parent");
        p_done.status = "done".into();
        be.update(p_done).expect("update parent");
        assert_eq!(status_of(a.id), "done");
    }

    #[test]
    fn query_open_only_and_due_filter() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());

        let with_due = |title: &str, status: &str, due: Option<&str>, sched: Option<&str>| {
            let mut t = crate::capture(title);
            t.status = status.into();
            t.due = due.map(str::to_owned);
            t.scheduled = sched.map(str::to_owned);
            be.create(t).expect("create");
        };
        with_due("overdue", "open", Some("2026-06-10"), None);
        with_due("today-sched", "open", None, Some("2026-06-14T09:00:00Z"));
        with_due("future", "open", Some("2026-06-20"), None);
        with_due("done-overdue", "done", Some("2026-06-01"), None); // terminal
        with_due("no-dates", "open", None, None);

        // open_only drops terminal-status tasks.
        let open = be
            .query(TaskListFilter {
                open_only: true,
                ..Default::default()
            })
            .expect("open only");
        assert_eq!(open.len(), 4);
        assert!(open.iter().all(|t| t.status != "done"));

        // due_on_or_before counts due OR scheduled, by date prefix, and
        // composes with open_only (the terminal overdue task is excluded).
        let due = be
            .query(TaskListFilter {
                open_only: true,
                due_on_or_before: Some("2026-06-14".into()),
                ..Default::default()
            })
            .expect("due");
        let titles: std::collections::HashSet<_> = due.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["overdue", "today-sched"].into_iter().collect());
    }

    #[test]
    fn query_filters_and_paginates_stably() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());
        let proj = Uuid::new_v4();
        let ws = Uuid::new_v4();
        make(&be, "alpha", "open", Some(proj), Some(ws));
        make(&be, "beta", "done", Some(proj), None);
        make(&be, "gamma", "open", None, Some(ws));
        make(&be, "delta", "open", None, None);

        // Default filter == list (modulo path ordering).
        let all = be.query(TaskListFilter::default()).expect("query all");
        assert_eq!(all.len(), 4);
        let mut paths: Vec<_> = all.iter().map(|t| t.path.clone()).collect();
        let sorted = paths.clone();
        paths.sort();
        assert_eq!(paths, sorted, "query orders by path");

        // Single filters.
        let by_proj = be
            .query(TaskListFilter {
                project: Some(proj),
                ..Default::default()
            })
            .expect("by project");
        assert_eq!(by_proj.len(), 2);
        assert!(by_proj.iter().all(|t| t.project_id == Some(proj)));

        let by_ws = be
            .query(TaskListFilter {
                workstream: Some(ws),
                ..Default::default()
            })
            .expect("by workstream");
        assert_eq!(by_ws.len(), 2);

        // Filters AND together; status is case-insensitive.
        let combo = be
            .query(TaskListFilter {
                project: Some(proj),
                status: Some("OPEN".into()),
                ..Default::default()
            })
            .expect("combo");
        assert_eq!(combo.len(), 1);
        assert_eq!(combo[0].title, "alpha");

        // Pagination windows tile the full ordered set.
        let page1 = be
            .query(TaskListFilter {
                limit: Some(2),
                ..Default::default()
            })
            .expect("page1");
        let page2 = be
            .query(TaskListFilter {
                limit: Some(2),
                offset: Some(2),
                ..Default::default()
            })
            .expect("page2");
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 2);
        let tiled: Vec<_> = page1.iter().chain(&page2).map(|t| t.id).collect();
        let full: Vec<_> = all.iter().map(|t| t.id).collect();
        assert_eq!(tiled, full, "limit/offset pages tile the path-ordered list");

        // Offset past the end is an empty page, not an error.
        let past = be
            .query(TaskListFilter {
                offset: Some(99),
                ..Default::default()
            })
            .expect("past end");
        assert!(past.is_empty());
    }

    #[test]
    fn reverse_relations_batch_one_walk_many_answers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());
        let victim = make(&be, "victim", "open", None, None);
        let bystander = make(&be, "bystander", "open", None, None);
        // blocker declares a typed `blocks -> victim` edge.
        let mut blocker = crate::capture("blocker");
        blocker.workflow = Some(WorkflowAttrs {
            relations: RelationList(vec![Relation {
                kind: RelationKind::Blocks,
                target: victim.id,
            }]),
            ..Default::default()
        });
        let blocker = be.create(blocker).expect("create blocker");

        let ghost = Uuid::new_v4();
        let got = be
            .reverse_relations_batch(vec![victim.id, bystander.id, ghost])
            .expect("batch");
        assert_eq!(got.len(), 3, "one entry per requested id");
        assert_eq!(got[0].id, victim.id, "requested order preserved");
        assert_eq!(
            got[0].relations,
            vec![crate::relations::ReverseRelation {
                kind: RelationKind::Blocks,
                source: blocker.id,
            }]
        );
        assert!(got[1].relations.is_empty(), "no incoming edges");
        assert!(
            got[2].relations.is_empty(),
            "unknown id -> empty, not error"
        );

        // Parity with the single-id verb.
        let single = be.reverse_relations(victim.id).expect("single");
        assert_eq!(single, got[0].relations);
    }

    // t[verify attribution.immutable]
    // t[verify attribution.filter]
    #[test]
    fn created_by_is_write_once_and_filters() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());
        let mut t = crate::capture("Restring the Telecaster");
        t.created_by = Some("bot:atlas".into());
        t.created_by_name = Some("Atlas".into());
        let made = be.create(t).expect("create");
        be.create(crate::capture("Unattributed")).expect("create");

        // An update naming someone else — or no one — keeps the creator,
        // and the file says so on the next read.
        let mut edit = made.clone();
        edit.title = "Restring the Strat".into();
        edit.created_by = Some("user:mallory".into());
        edit.created_by_name = None;
        let after = be.update(edit).expect("update");
        assert_eq!(after.created_by.as_deref(), Some("bot:atlas"));
        let read = be.get(made.id).expect("get");
        assert_eq!(read.title, "Restring the Strat");
        assert_eq!(read.created_by.as_deref(), Some("bot:atlas"));
        assert_eq!(read.created_by_name.as_deref(), Some("Atlas"));

        let by = |who: &str| {
            be.query(TaskListFilter {
                created_by: Some(who.into()),
                ..Default::default()
            })
            .expect("query")
            .into_iter()
            .map(|t| t.title)
            .collect::<Vec<_>>()
        };
        assert_eq!(by("bot:atlas"), vec!["Restring the Strat".to_owned()]);
        assert_eq!(by("atlas"), vec!["Restring the Strat".to_owned()]);
        assert_eq!(by("none"), vec!["Unattributed".to_owned()]);
        assert!(by("user:mallory").is_empty());
    }

    // t[verify attribution.on-behalf-of]
    // t[verify attribution.assignee.filter]
    #[test]
    fn on_behalf_of_is_write_once_and_both_filters_work() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let be = TaskBackend::new(tmp.path());
        let mut t = crate::capture("Book the rehearsal room");
        t.created_by = Some("bot:atlas".into());
        t.on_behalf_of = Some("user:cody".into());
        t.on_behalf_of_name = Some("Cody".into());
        let made = be.create(t).expect("create");
        let mut theirs = crate::capture("Load the van");
        theirs
            .workflow
            .get_or_insert_with(Default::default)
            .assignees =
            task_proto::model::AgentRefList(vec![workflows_proto::AgentRef::human("jordan")]);
        be.create(theirs).expect("create");

        // Neither an edit naming someone else nor one leaving it out
        // moves whom it was made for — but the assignee is the edit's.
        let mut edit = made.clone();
        edit.on_behalf_of = Some("user:mallory".into());
        edit.on_behalf_of_name = None;
        edit.workflow.get_or_insert_with(Default::default).assignees =
            task_proto::model::AgentRefList(vec![workflows_proto::AgentRef::human("jordan")]);
        be.update(edit).expect("update");
        let read = be.get(made.id).expect("get");
        assert_eq!(read.on_behalf_of.as_deref(), Some("user:cody"));
        assert_eq!(read.on_behalf_of_name.as_deref(), Some("Cody"));
        assert_eq!(read.created_by.as_deref(), Some("bot:atlas"));

        let titles = |filter: TaskListFilter| {
            let mut v: Vec<String> = be
                .query(filter)
                .expect("query")
                .into_iter()
                .map(|t| t.title)
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            titles(TaskListFilter {
                on_behalf_of: Some("cody".into()),
                ..Default::default()
            }),
            vec!["Book the rehearsal room".to_owned()]
        );
        assert_eq!(
            titles(TaskListFilter {
                on_behalf_of: Some("none".into()),
                ..Default::default()
            }),
            vec!["Load the van".to_owned()]
        );
        assert_eq!(
            titles(TaskListFilter {
                assignee: Some("user:jordan".into()),
                ..Default::default()
            }),
            vec![
                "Book the rehearsal room".to_owned(),
                "Load the van".to_owned()
            ]
        );
        assert!(
            titles(TaskListFilter {
                assignee: Some("none".into()),
                ..Default::default()
            })
            .is_empty()
        );
    }
}
