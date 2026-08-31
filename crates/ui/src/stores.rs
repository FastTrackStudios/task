//! Per-feature optimistic stores — the ONE state pattern for route pages.
//!
//! Built on `architect-atom` (`Store` + `AtomResult` + `use_mutation`),
//! mirroring the architect example app's derived-store idioms by hand
//! (Task's reads/writes are slug-routed through the multi-org
//! [`crate::feeds`] fan-out, which the derive can't know):
//!
//! - one shared [`architect::Store`] per entity, provided at the app
//!   root by [`provide_stores`];
//! - `use_<entity>_list()` — the rows as one [`AtomResult`]
//!   (stale-while-revalidate: an org switch shows the last data while
//!   the refetch is in flight, never a blank "Loading…");
//! - `use_<entity>_mutations()` — optimistic writes through
//!   [`architect::Mutation::run`]: the store is patched instantly
//!   (typed `Id::Temp` placeholders, no magic id sentinels), then
//!   reconciled against the server's row or rolled back, with failures
//!   reported to the app's `Notifications` tray.
//!
//! Entities that live in exactly one org at a time (locations,
//! inventory, …) store the proto type directly; multi-org views (tasks,
//! projects, timer sessions, invoices) wrap rows in `Org<X>` pairs so a
//! mutation can route back to the owning org's service.
//!
//! Supersedes the in-house optimistic write-through list helper
//! (`src/optimistic.rs`), the task wiring shim, and the
//! refresh-counter pages.
//!
//! # The pattern lives in `task-stores`
//!
//! The `stores!` table, the multi-org fan-out and the live-subscription
//! driver moved to [`task_stores`], which names no domain at all. This
//! file is now only Task's *own* stores — the ones the shell mounts
//! directly.
//!
//! A feature that becomes an app takes its rows with it: it invokes
//! `task_stores::stores!` in its own crate and declares `provide` on
//! its `PluginApp`, and the shell calls that alongside this module's
//! [`provide_stores`]. That is what the split was for — a plugin crate
//! cannot depend on `task-ui`, so while the pattern lived here, an app
//! could not reach the pattern its own page was written against.

use std::collections::HashSet;

use architect::{
    AtomResult, Id, Mutation, StoreEntity, use_mutation, use_store_entry, use_store_list,
};
use chrono::{Datelike, NaiveDate, Utc, Weekday};
use dioxus::prelude::*;
use project_proto::ProjectInfo;
use task_proto::TaskInfo as DbTask;
use task_ui::TaskMutation;
use timer_proto::{StartTimerRequest, WorkSession};
use uuid::Uuid;

use task_stores::{run_create, use_multi_org_list, use_org_scope};

task_stores::stores! {
    TaskStore: OrgTask {
        provide: provide_task_store,
        handle: use_task_store,
        stream:
            /// Live board: every `TaskEvent` from every selected org
            /// folds into the shared store, so the `/tasks` board and
            /// each project-detail board update without refetching.
            all task_proto::TaskServiceStreamClient => fold_task_event,
    }

    ProjectStore: OrgProject {
        provide: provide_project_store,
        handle: use_project_store,
        stream:
            /// Live project axis — Upserted/Deleted `ProjectEvent`s
            /// keep `/projects` and the board's grouping current.
            all project_proto::ProjectServiceStreamClient => fold_project_event,
        mutations:
            /// Optimistic writes for projects (full-record update).
            ProjectMutations via use_project_mutations,
    }

    GoalStore: OrgGoal {
        provide: provide_goal_store,
        handle: use_goal_store,
        stream:
            /// Live goal hierarchy — Upserted/Deleted `GoalEvent`s
            /// from every selected org fold into the merged list.
            all goal_proto::GoalServiceStreamClient => fold_goal_event,
    }

    MilestoneStore: milestone_proto::Milestone {
        provide: provide_milestone_store,
        handle: use_milestone_store,
        list: use_milestone_list -> Uuid = crate::feeds::fetch_milestones,
        stream: first milestone_proto::MilestoneServiceStreamClient => fold_milestone_event,
        mutations: MilestoneMutations via use_milestone_mutations,
    }

    InboxStore: inbox_proto::InboxItem {
        provide: provide_inbox_store,
        handle: use_inbox_store,
        list: use_inbox_list -> String = crate::feeds::fetch_inbox,
        stream:
            /// Live capture queue — an item captured on another
            /// device (CLI, watch, phone) appears without a refetch.
            first inbox_proto::InboxStreamClient => fold_inbox_event,
        mutations:
            /// Optimistic writes for the capture queue. Upserts return unit on the
            /// wire, so the row we sent doubles as the reconciled value (the id is
            /// client-minted and stable).
            InboxMutations via use_inbox_mutations,
    }

    NotificationStore: notify_proto::Notification {
        provide: provide_notification_store,
        handle: use_notification_store,
        list: use_notification_list -> Uuid = crate::feeds::fetch_notifications,
        stream:
            /// Live bell — the notifier's pushes and read-flips fold
            /// in as they happen, so the unread badge is real-time.
            first notify_proto::NotifyStreamClient => fold_notify_event,
    }

    ContactStore: contacts_proto::Contact {
        provide: provide_contact_store,
        handle: use_contact_store,
        list: use_contact_list -> String = crate::feeds::fetch_contacts,
        stream:
            /// Live directory — a CardDAV sync run's writes stream in
            /// card by card.
            first contacts_proto::ContactsStreamClient => fold_contact_event,
        mutations:
            /// Optimistic writes for the directory. Upserts return unit on the
            /// wire, so the row we sent doubles as the reconciled value (the id is
            /// client-minted and stable).
            ContactMutations via use_contact_mutations,
    }

    DayPlanStore: DayPlanRow {
        provide: provide_dayplan_store,
        handle: use_dayplan_store,
        mutations:
            /// Optimistic writes for the schedule's per-day plans: every edit
            /// patches the date's store row synchronously, then persists the whole
            /// merged day (`save_day_plan`); a rejected write rolls the day back
            /// and reports to the Notifications tray.
            DayPlanMutations via use_dayplan_mutations,
    }

    SessionStore: OrgSession {
        provide: provide_session_store,
        handle: use_session_store,
        stream:
            /// Live sessions: the running timer is *derived* (the
            /// open row), so folding `TimerEvent`s makes another
            /// device's start/stop/switch show up instantly.
            all timer_proto::TimerServiceStreamClient => fold_timer_event,
        mutations: TimerMutations via use_timer_mutations,
    }

    ThreadStore: threads::Thread {
        provide: provide_thread_store,
        handle: use_thread_store,
    }

    MessageStore: threads::Message {
        provide: provide_message_store,
        handle: use_message_store,
    }
}

// ── shared plumbing ─────────────────────────────────────────────────

/// The org-selection contexts every list hook keys its fetch off.

fn fold_task_event(store: &TaskStore, slug: &str, ev: task_proto::TaskEvent) {
    match ev {
        task_proto::TaskEvent::Upserted(task) => store.put(OrgTask {
            slug: slug.to_owned(),
            task,
        }),
        task_proto::TaskEvent::Deleted(id) => store.remove_real(&id),
    }
}

fn fold_project_event(store: &ProjectStore, slug: &str, ev: project_proto::ProjectEvent) {
    match ev {
        project_proto::ProjectEvent::Upserted(project) => store.put(OrgProject {
            slug: slug.to_owned(),
            project,
        }),
        project_proto::ProjectEvent::Deleted(id) => store.remove_real(&id),
    }
}

fn fold_goal_event(store: &GoalStore, slug: &str, ev: goal_proto::GoalEvent) {
    match ev {
        goal_proto::GoalEvent::Upserted(goal) => store.put(OrgGoal {
            slug: slug.to_owned(),
            goal,
        }),
        goal_proto::GoalEvent::Deleted(id) => store.remove_real(&id),
    }
}

fn fold_milestone_event(store: &MilestoneStore, _slug: &str, ev: milestone_proto::MilestoneEvent) {
    match ev {
        milestone_proto::MilestoneEvent::Upserted(ms) => store.put(ms),
        milestone_proto::MilestoneEvent::Deleted(id) => store.remove_real(&id),
    }
}

fn fold_inbox_event(store: &InboxStore, _slug: &str, ev: inbox_proto::InboxEvent) {
    match ev {
        inbox_proto::InboxEvent::Upserted(item) => store.put(item),
        inbox_proto::InboxEvent::Deleted(id) => store.remove_real(&id),
    }
}

fn fold_notify_event(store: &NotificationStore, _slug: &str, ev: notify_proto::NotifyEvent) {
    match ev {
        notify_proto::NotifyEvent::Upserted(n) => store.put(n),
        notify_proto::NotifyEvent::Deleted(id) => store.remove_real(&id),
    }
}

fn fold_contact_event(store: &ContactStore, _slug: &str, ev: contacts_proto::ContactsEvent) {
    match ev {
        contacts_proto::ContactsEvent::Upserted(contact) => store.put(contact),
        contacts_proto::ContactsEvent::Deleted(id) => store.remove_real(&id),
    }
}

fn fold_timer_event(store: &SessionStore, slug: &str, ev: timer_proto::TimerEvent) {
    match ev {
        timer_proto::TimerEvent::Upserted(session) => store.put(OrgSession {
            slug: slug.to_owned(),
            session,
        }),
        timer_proto::TimerEvent::Deleted(id) => store.remove_real(&id),
    }
}

/// Store-backed list scoped to the **first selected org** (or home) —
/// the shape of the single-org register pages. Re-fetches when the org
/// switcher moves (the closure reads the selection signals); `None`
/// (discovery pending) keeps the phase at `Loading`.
#[allow(clippy::type_complexity)] // `AtomResult<Vec<(Id, T)>, _>` reads fine.
#[derive(Clone, PartialEq)]
pub struct OrgTask {
    pub slug: String,
    pub task: DbTask,
}

impl StoreEntity for OrgTask {
    type Key = Uuid;
    fn key(&self) -> Uuid {
        self.task.id
    }
}

/// Tasks across the selected orgs as one [`AtomResult`].
pub fn use_task_list() -> AtomResult<Vec<(Id<Uuid>, OrgTask)>, String> {
    use_multi_org_list(use_task_store(), |slugs| async move {
        crate::feeds::fetch_tasks_tagged(&slugs).await.map(|rows| {
            rows.into_iter()
                .map(|(slug, task)| OrgTask { slug, task })
                .collect()
        })
    })
}

/// Optimistic writes for the task board, keyed off the store's slug tag.
#[derive(Clone, Copy)]
pub struct TaskMutations {
    store: TaskStore,
    write: Mutation<String>,
    /// When set, quick-added tasks are filed under this project —
    /// the project detail page scopes its board so "add a task here"
    /// means *here*.
    create_project_scope: Option<Uuid>,
    /// Project store handle (Copy — it's a signal), for resolving
    /// `[[Project]]` wikilinks typed into quick-add
    /// (task_proto::infer_project_id) at create time.
    projects: ProjectStore,
}

pub fn use_task_mutations() -> TaskMutations {
    TaskMutations {
        store: use_task_store(),
        write: use_mutation(),
        create_project_scope: None,
        projects: use_context(),
    }
}

impl TaskMutations {
    /// Scope quick-add creates to a project (see
    /// [`Self::create_project_scope`]). Builder-style so the project
    /// page writes `use_task_mutations().scoped_to(project_id)`.
    #[must_use]
    pub fn scoped_to(mut self, project_id: Uuid) -> Self {
        self.create_project_scope = Some(project_id);
        self
    }
}

impl TaskMutations {
    /// Apply one `task_ui` board mutation: optimistic store patch +
    /// write-through to the owning org (`create_slug` for new rows).
    pub fn apply(&self, create_slug: &str, mu: TaskMutation) {
        match mu {
            TaskMutation::Create { task } => {
                let mut draft = task_proto::capture(&task.title);
                // Filing priority: explicit picker > page scope
                // (project detail board) > [[wikilink]] inference.
                let known: Vec<(Uuid, String)> = self
                    .projects
                    .list()
                    .into_iter()
                    .map(|r| (r.project.id, r.project.title))
                    .collect();
                draft.project_id = task
                    .project_id
                    .or(self.create_project_scope)
                    .or_else(|| task_proto::infer_project_id(&draft.projects.0, &known));
                self.create(OrgTask {
                    slug: create_slug.to_owned(),
                    task: draft,
                });
            }
            TaskMutation::Update { task } => {
                // The board renders the authoritative record, so the
                // detail sheet hands back a complete row — whole-record
                // replace, no field-by-field bridge to keep in sync.
                let id = task.id;
                self.edit(id, move |t| t.clone_from(&task));
            }
            TaskMutation::SetStatus { id, status } => {
                // Optimistic family cascade — the same domain rule the
                // server applies (task_proto::cascade_status), computed from
                // the pre-edit store + the incoming status so parent and
                // subtask checkboxes flip together instantly. The
                // server's own cascade reconciles to the identical state
                // (re-running the rule on an already-cascaded family is
                // a no-op).
                let follow_ups = self
                    .store
                    .get_real(id)
                    .map(|row| {
                        let mut changed = row.task.clone();
                        changed.status.clone_from(&status);
                        let all: Vec<DbTask> =
                            self.store.list().into_iter().map(|r| r.task).collect();
                        task_proto::cascade_status(&all, &changed)
                    })
                    .unwrap_or_default();
                self.edit(id, move |t| {
                    let prev = t.status.clone();
                    t.status = status;
                    // Mirror the server's automatic time tracking so the
                    // running-timer state shows instantly.
                    task_proto::track_status_transition(&prev, t, chrono::Utc::now());
                });
                for (fid, fstatus) in follow_ups {
                    self.edit(fid, move |t| {
                        let prev = t.status.clone();
                        t.status = fstatus;
                        task_proto::track_status_transition(&prev, t, chrono::Utc::now());
                    });
                }
            }
            TaskMutation::SetPriority { id, priority } => {
                self.edit(id, move |t| t.priority = priority);
            }
            TaskMutation::Delete { id } => self.delete(id),
        }
    }

    fn create(&self, row: OrgTask) {
        run_create(self.write, self.store, row, |row| async move {
            let slug = row.slug;
            let client = crate::vox_clients::task_client(&slug).await?;
            client
                .create(row.task)
                .await
                .map_err(|e| format!("{slug}: create task: {e:?}"))
                .map(|task| OrgTask { slug, task })
        });
    }

    fn edit(&self, id: Uuid, patch: impl FnOnce(&mut DbTask)) {
        // Patch a snapshot of the current row (full-record service API),
        // then write the whole record through to its owning org.
        let Some(mut next) = self.store.get_real(id) else {
            return;
        };
        patch(&mut next.task);
        let row = next.clone();
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(id), move |r| *r = row),
            move || async move {
                let slug = next.slug;
                let client = crate::vox_clients::task_client(&slug).await?;
                client
                    .update(next.task)
                    .await
                    .map_err(|e| format!("{slug}: update task: {e:?}"))
                    .map(|task| Some(OrgTask { slug, task }))
            },
        );
    }

    fn delete(&self, id: Uuid) {
        let Some(row) = self.store.get_real(id) else {
            return;
        };
        let slug = row.slug;
        self.write.run(
            self.store,
            move |s| s.remove_optimistic(Id::Real(id)),
            move || async move {
                let client = crate::vox_clients::task_client(&slug).await?;
                client
                    .delete(id)
                    .await
                    .map(|()| None)
                    .map_err(|e| format!("{slug}: delete task: {e:?}"))
            },
        );
    }
}

// ── projects (multi-org, slug-tagged) ───────────────────────────────

/// One project tagged with its owning org's slug.
#[derive(Clone, PartialEq)]
pub struct OrgProject {
    pub slug: String,
    pub project: ProjectInfo,
}

impl StoreEntity for OrgProject {
    type Key = Uuid;
    fn key(&self) -> Uuid {
        self.project.id
    }
}

/// Projects across the selected orgs as one [`AtomResult`]. Hydrates
/// the shared store, so `/projects/:id` is instant after a list visit.
pub fn use_project_list() -> AtomResult<Vec<(Id<Uuid>, OrgProject)>, String> {
    use_multi_org_list(use_project_store(), |slugs| async move {
        crate::feeds::fetch_projects_tagged(&slugs)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|(slug, project)| OrgProject { slug, project })
                    .collect()
            })
    })
}

fn parse_project_id(raw: &str) -> Result<Uuid, String> {
    raw.parse().map_err(|_| "invalid project id".to_owned())
}

/// One project by route id — cache-first (`Success` straight from the
/// store after a `/projects` visit), else a per-org `get` probe across
/// the selected orgs (no whole-list refetch).
pub fn use_project(id: String) -> AtomResult<OrgProject, String> {
    let store = use_project_store();
    let (selection, orgs) = use_org_scope();
    use_store_entry(store, id, parse_project_id, move |key| {
        let slugs = crate::orgs::selected_slugs(&selection.read(), &orgs.read());
        async move {
            if slugs.is_empty() {
                return None; // discovery pending → Loading
            }
            Some(
                crate::feeds::find_project(&key.to_string(), &slugs)
                    .await
                    .map(|(project, slug)| OrgProject { slug, project }),
            )
        }
    })
}

/// A new project from just a title — active, normal priority, the
/// backend fills id/path/dates. The one draft literal (CLI has its
/// own flag-driven builder).
#[must_use]
pub fn draft_project(title: String) -> ProjectInfo {
    ProjectInfo {
        id: Uuid::nil(),
        title,
        project_type: "general".into(),
        ..Default::default()
    }
}

impl ProjectMutations {
    /// True while a project write is in flight.
    pub fn is_pending(&self) -> bool {
        self.write.is_pending()
    }

    /// Optimistically insert a new project (see [`draft_project`]),
    /// then create it through the org's `ProjectService`.
    pub fn create(&self, slug: String, draft: ProjectInfo) {
        let row = OrgProject {
            slug: slug.clone(),
            project: draft,
        };
        run_create(self.write, self.store, row, |row| async move {
            let slug = row.slug;
            crate::feeds::create_project(&slug, row.project)
                .await
                .map(|project| OrgProject { slug, project })
        });
    }

    /// The last failed write's error, for inline display.
    pub fn error(&self) -> Option<String> {
        self.write.error()
    }

    /// Optimistically replace the project record, then write through to
    /// its org's `ProjectService` (markdown frontmatter).
    pub fn update(&self, slug: String, project: ProjectInfo) {
        let id = project.id;
        let row = OrgProject {
            slug: slug.clone(),
            project: project.clone(),
        };
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(id), move |r| *r = row),
            move || async move {
                crate::feeds::update_project(&slug, project)
                    .await
                    .map(|project| Some(OrgProject { slug, project }))
            },
        );
    }
}

// ── goals (multi-org, slug-tagged) ──────────────────────────────────

/// One goal tagged with its owning org's slug — the tag keys the
/// live stream fold (and any future write-back), like [`OrgTask`].
#[derive(Clone, PartialEq)]
pub struct OrgGoal {
    pub slug: String,
    pub goal: goal_proto::Goal,
}

impl StoreEntity for OrgGoal {
    type Key = Uuid;
    fn key(&self) -> Uuid {
        self.goal.id
    }
}

/// Goals across the selected orgs as one [`AtomResult`]. The `/goals`
/// page renders the merged hierarchy from this store, which the
/// app-root `GoalService` stream subscription keeps live.
pub fn use_goal_list() -> AtomResult<Vec<(Id<Uuid>, OrgGoal)>, String> {
    use_multi_org_list(use_goal_store(), |slugs| async move {
        crate::feeds::fetch_goals_tagged(&slugs).await.map(|rows| {
            rows.into_iter()
                .map(|(slug, goal)| OrgGoal { slug, goal })
                .collect()
        })
    })
}

// ── milestones ──────────────────────────────────────────────────────

/// Unsaved placeholder row for an optimistic milestone insert.
pub fn draft_milestone(
    title: String,
    project_id: Uuid,
    due_date: Option<chrono::NaiveDate>,
) -> milestone_proto::Milestone {
    milestone_proto::Milestone {
        path: String::new(),
        id: Uuid::nil(),
        title,
        project_id,
        goal_id: None,
        status: "open".to_owned(),
        due_date,
        tags: milestone_proto::Tags::default(),
        forge_ref: None,
        date_created: None,
        date_modified: None,
        details: String::new(),
    }
}

impl MilestoneMutations {
    pub fn create(&self, slug: String, draft: milestone_proto::Milestone) {
        run_create(self.write, self.store, draft, move |ms| async move {
            crate::feeds::create_milestone(&slug, ms).await
        });
    }
}

// ── inbox ───────────────────────────────────────────────────────────

impl InboxMutations {
    /// Capture a fresh item: it appears instantly, then persists.
    pub fn capture(&self, slug: String, item: inbox_proto::InboxItem) {
        run_create(self.write, self.store, item, move |item| async move {
            crate::feeds::upsert_inbox_item(&slug, item.clone())
                .await
                .map(|()| item)
        });
    }

    /// Replace an item in place (status flips, snoozes), then persist.
    pub fn save(&self, slug: String, next: inbox_proto::InboxItem) {
        let id = next.id.clone();
        let row = next.clone();
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(id), move |r| *r = row),
            move || async move {
                crate::feeds::upsert_inbox_item(&slug, next.clone())
                    .await
                    .map(|()| Some(next))
            },
        );
    }

    /// Remove an item now; restore (and notify) if the delete fails.
    pub fn delete(&self, slug: String, id: String) {
        let key = id.clone();
        self.write.run(
            self.store,
            move |s| s.remove_optimistic(Id::Real(key)),
            move || async move {
                crate::feeds::delete_inbox_item(&slug, &id)
                    .await
                    .map(|()| None)
            },
        );
    }

    /// Promote an item into a Task: optimistically mark it processed,
    /// then create the task and persist the provenance back-link.
    pub fn promote_to_task(
        &self,
        slug: String,
        item: inbox_proto::InboxItem,
        title: String,
        details: String,
    ) {
        let mut done = item;
        done.status = inbox_proto::InboxItem::STATUS_PROCESSED.to_string();
        let id = done.id.clone();
        let row = done.clone();
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(id), move |r| *r = row),
            move || async move {
                let task = crate::feeds::create_task(&slug, &title, &details).await?;
                let mut done = done;
                done.processed_into = Some(task.path);
                crate::feeds::upsert_inbox_item(&slug, done.clone())
                    .await
                    .map(|()| Some(done))
            },
        );
    }

    /// Promote an item into an atomic wiki note (same shape as
    /// [`Self::promote_to_task`], writing markdown to the vault).
    pub fn promote_to_note(
        &self,
        slug: String,
        item: inbox_proto::InboxItem,
        path: String,
        markdown: String,
    ) {
        let mut done = item;
        done.status = inbox_proto::InboxItem::STATUS_PROCESSED.to_string();
        let id = done.id.clone();
        let row = done.clone();
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(id), move |r| *r = row),
            move || async move {
                crate::feeds::create_wiki_note(&slug, &path, &markdown).await?;
                let mut done = done;
                done.processed_into = Some(path);
                crate::feeds::upsert_inbox_item(&slug, done.clone())
                    .await
                    .map(|()| Some(done))
            },
        );
    }
}

// ── contacts (vault-backed people directory) ────────────────────────

impl ContactMutations {
    /// Author a fresh contact: it appears instantly, then persists.
    pub fn create(&self, slug: String, contact: contacts_proto::Contact) {
        run_create(self.write, self.store, contact, move |contact| async move {
            crate::feeds::upsert_contact(&slug, contact.clone())
                .await
                .map(|()| contact)
        });
    }

    /// Replace a contact in place (edits, link, archive), then persist.
    pub fn save(&self, slug: String, next: contacts_proto::Contact) {
        let id = next.id.clone();
        let row = next.clone();
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(id), move |r| *r = row),
            move || async move {
                crate::feeds::upsert_contact(&slug, next.clone())
                    .await
                    .map(|()| Some(next))
            },
        );
    }

    /// Remove a contact now; restore (and notify) if the delete fails.
    pub fn delete(&self, slug: String, id: String) {
        let key = id.clone();
        self.write.run(
            self.store,
            move |s| s.remove_optimistic(Id::Real(key)),
            move || async move {
                crate::feeds::delete_contact(&slug, &id)
                    .await
                    .map(|()| None)
            },
        );
    }
}

// ── day plans (the /schedule calendar's per-date plans) ─────────────

/// One date's resolved plan as `/schedule` renders it: the saved
/// [`DayPlan`](scheduling_proto::DayPlan) merged with the date's
/// weekday/weekend template, plus which block ids are *soft* —
/// template fallback rather than explicitly saved (rendered
/// dashed/faded). A save persists the whole merged day and clears the
/// soft set: every block is explicit from there on.
#[derive(Clone, PartialEq)]
pub struct DayPlanRow {
    pub date: NaiveDate,
    pub plan: scheduling_proto::DayPlan,
    pub soft_ids: HashSet<String>,
}

impl StoreEntity for DayPlanRow {
    type Key = NaiveDate;
    fn key(&self) -> NaiveDate {
        self.date
    }
}

/// The template a date defaults to: `weekend` for Sat/Sun, else
/// `weekday`; falls back to the first template if those ids are absent.
fn template_for(
    date: NaiveDate,
    templates: &[scheduling_proto::DayTemplate],
) -> Option<&scheduling_proto::DayTemplate> {
    let weekend = matches!(date.weekday(), Weekday::Sat | Weekday::Sun);
    let want = if weekend { "weekend" } else { "weekday" };
    templates
        .iter()
        .find(|t| t.id.0 == want)
        .or_else(|| templates.first())
}

/// Resolve a date to the row the page renders: the saved plan's blocks
/// verbatim (explicit) plus the matching template's blocks reflowed
/// into the remaining free time as *soft* fallback (see
/// `scheduling_proto::resolve::merge_template`). A date with no saved
/// plan is the all-soft template; a *sparse* saved plan (e.g. only a
/// scheduled meal block) still shows the rest of its day.
fn resolve_day(
    date: NaiveDate,
    saved: Option<scheduling_proto::DayPlan>,
    templates: &[scheduling_proto::DayTemplate],
) -> DayPlanRow {
    let tpl = template_for(date, templates);
    let saved_blocks: &[scheduling_proto::PlannedBlock] =
        saved.as_ref().map_or(&[], |p| &p.blocks.0);
    let tpl_blocks: &[scheduling_proto::TimeBlock] = tpl.map_or(&[], |t| &t.blocks.0);
    let merged = scheduling_proto::resolve::merge_template(saved_blocks, tpl_blocks);
    let soft_ids: HashSet<String> = merged
        .iter()
        .filter(|rb| rb.soft)
        .map(|rb| rb.block.id.0.clone())
        .collect();
    let plan = scheduling_proto::DayPlan {
        date: date.to_string(),
        from_template: saved
            .and_then(|p| p.from_template)
            .or_else(|| tpl.map(|t| t.id.clone())),
        blocks: merged.into_iter().map(|rb| rb.block).collect(),
    };
    DayPlanRow {
        date,
        plan,
        soft_ids,
    }
}

/// Resolved plans for every date in the visible calendar `range`,
/// materialized against the org's day `templates` (both owned by the
/// page — `None` while either is still resolving keeps the phase at
/// `Loading`). Scoped to the first selected org; navigating the
/// calendar or switching orgs refetches the range while the last rows
/// stay rendered (`Reloading`), never a blank calendar.
#[allow(clippy::type_complexity)] // `AtomResult<Vec<(Id, T)>, _>` reads fine.
pub fn use_dayplan_list(
    range: Option<(NaiveDate, NaiveDate)>,
    templates: Option<Vec<scheduling_proto::DayTemplate>>,
) -> AtomResult<Vec<(Id<NaiveDate>, DayPlanRow)>, String> {
    let (selection, orgs) = use_org_scope();
    let store = use_dayplan_store();
    let key = use_memo(use_reactive!(|(range, templates)| (range, templates)));
    use_store_list(store, move || {
        let slug = crate::orgs::selected_slugs(&selection.read(), &orgs.read())
            .into_iter()
            .next();
        let (range, templates) = key();
        let pending = match (slug, range, templates) {
            (Some(slug), Some((start, end)), Some(tpls)) => Some((slug, start, end, tpls)),
            _ => None,
        };
        async move {
            let (slug, start, end, tpls) = pending?;
            let mut rows = Vec::new();
            let mut d = start;
            while d <= end {
                let saved = match crate::feeds::fetch_day_plan(&slug, &d.to_string()).await {
                    Ok(saved) => saved,
                    Err(e) => return Some(Err(e)),
                };
                rows.push(resolve_day(d, saved, &tpls));
                let Some(next) = d.succ_opt() else { break };
                d = next;
            }
            Some(Ok(rows))
        }
    })
}

fn clamp_time(min: u16) -> scheduling_proto::TimeOfDay {
    scheduling_proto::TimeOfDay {
        minutes_since_midnight: min.min(1440),
    }
}

impl DayPlanMutations {
    /// The shared save lifecycle: patch one date's plan, mark every
    /// block explicit (clear the soft set — the save persists the whole
    /// merged day), write it through.
    fn save_day(
        &self,
        slug: String,
        date: NaiveDate,
        patch: impl FnOnce(&mut scheduling_proto::DayPlan),
    ) {
        let Some(mut row) = self.store.get_real(date) else {
            return;
        };
        patch(&mut row.plan);
        row.soft_ids.clear();
        let plan = row.plan.clone();
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(date), move |r| *r = row),
            move || async move {
                crate::feeds::save_day_plan(&slug, plan)
                    .await
                    .map(|()| None)
            },
        );
    }

    /// Relabel / retime / reassign one block (the block editor's Save).
    pub fn save_block(
        &self,
        slug: String,
        date: NaiveDate,
        id: String,
        label: String,
        minutes: (u16, u16),
        assignment: Option<scheduling_proto::BlockAssignment>,
    ) {
        self.save_day(slug, date, move |plan| {
            if let Some(b) = plan.blocks.0.iter_mut().find(|b| b.id.0 == id) {
                b.label = label;
                b.start = clamp_time(minutes.0);
                b.end = clamp_time(minutes.1);
                b.assignment = assignment;
            }
        });
    }

    /// Set just a block's assignment (drag-a-task-onto-a-block).
    pub fn assign_block(
        &self,
        slug: String,
        date: NaiveDate,
        id: String,
        assignment: Option<scheduling_proto::BlockAssignment>,
    ) {
        self.save_day(slug, date, move |plan| {
            if let Some(b) = plan.blocks.0.iter_mut().find(|b| b.id.0 == id) {
                b.assignment = assignment;
            }
        });
    }

    /// Move/retime a block from a grid drag, possibly across days;
    /// each affected day persists (and rolls back) independently.
    pub fn move_block(
        &self,
        slug: String,
        orig: NaiveDate,
        target: NaiveDate,
        id: String,
        minutes: (u16, u16),
    ) {
        let (start, end) = (clamp_time(minutes.0), clamp_time(minutes.1));
        if orig == target {
            self.save_day(slug, orig, move |plan| {
                if let Some(b) = plan.blocks.0.iter_mut().find(|b| b.id.0 == id) {
                    b.start = start;
                    b.end = end;
                }
            });
            return;
        }
        // Cross-day: pull the block out of its origin day and drop it
        // onto the target (both rows must be loaded).
        let Some(src) = self.store.get_real(orig) else {
            return;
        };
        if self.store.get_real(target).is_none() {
            return;
        }
        let Some(mut blk) = src.plan.blocks.0.iter().find(|b| b.id.0 == id).cloned() else {
            return;
        };
        blk.start = start;
        blk.end = end;
        {
            let id = id.clone();
            self.save_day(slug.clone(), orig, move |plan| {
                plan.blocks.0.retain(|b| b.id.0 != id);
            });
        }
        self.save_day(slug, target, move |plan| plan.blocks.0.push(blk));
    }

    /// Revert a date to its template: the all-soft day re-materializes
    /// instantly, the saved plan behind it is deleted.
    pub fn reset_day(
        &self,
        slug: String,
        date: NaiveDate,
        templates: &[scheduling_proto::DayTemplate],
    ) {
        let row = resolve_day(date, None, templates);
        self.write.run(
            self.store,
            move |s| s.update_optimistic(Id::Real(date), move |r| *r = row),
            move || async move {
                crate::feeds::delete_day_plan(&slug, &date.to_string())
                    .await
                    .map(|()| None)
            },
        );
    }
}

// ── timer sessions (multi-org, slug-tagged) ─────────────────────────

/// One work session tagged with its owning org's slug.
#[derive(Clone, PartialEq)]
pub struct OrgSession {
    pub slug: String,
    pub session: WorkSession,
}

impl StoreEntity for OrgSession {
    type Key = Uuid;
    fn key(&self) -> Uuid {
        self.session.id
    }
}

/// Sessions across the selected orgs, newest first. The running timer
/// is *derived* from this one list (the open row for the active org +
/// owner) — the single-record `active_timer` round-trip is gone.
pub fn use_session_list() -> AtomResult<Vec<(Id<Uuid>, OrgSession)>, String> {
    use_multi_org_list(use_session_store(), |slugs| async move {
        Ok(crate::feeds::fetch_sessions_multi(&slugs)
            .await
            .into_iter()
            .map(|(slug, session)| OrgSession { slug, session })
            .collect())
    })
}

/// Unsaved placeholder for an optimistic timer start. `billable` /
/// rate are server-snapshotted; the reconcile swaps in the truth.
pub fn draft_session(req: &StartTimerRequest) -> WorkSession {
    let now = Utc::now();
    WorkSession {
        id: Uuid::nil(),
        org_id: req.org_id,
        user_id: req.user_id,
        project_id: req.project_id,
        project_path: req.project_path.clone(),
        description: req.description.clone(),
        start_time: now,
        end_time: None,
        billable: false,
        rate_cents: 0,
        currency: String::new(),
        task_note_path: req.task_note_path.clone(),
        invoice_id: None,
        created_at: now,
        updated_at: now,
    }
}

impl TimerMutations {
    /// Start a timer: an open session appears instantly, then
    /// reconciles to the server's row.
    pub fn start(&self, slug: String, req: StartTimerRequest) {
        let row = OrgSession {
            slug: slug.clone(),
            session: draft_session(&req),
        };
        run_create(self.write, self.store, row, move |_| async move {
            crate::feeds::start_timer(&slug, req)
                .await
                .map(|session| OrgSession {
                    slug: slug.clone(),
                    session,
                })
        });
    }

    /// Stop the running session: it closes instantly (end = now), then
    /// reconciles to the server's closed row (authoritative end + rate).
    pub fn stop(&self, slug: String, user_id: Uuid, session_id: Uuid) {
        self.write.run(
            self.store,
            move |s| {
                s.update_optimistic(Id::Real(session_id), |r| {
                    r.session.end_time = Some(Utc::now());
                })
            },
            move || async move {
                crate::feeds::stop_timer(&slug, user_id)
                    .await
                    .map(|session| Some(OrgSession { slug, session }))
            },
        );
    }

    /// Switch the running timer to a new task in one atomic step: the
    /// open row closes and a fresh open session appears instantly, then
    /// both reconcile against the server's `switch_timer`. `open_id` is
    /// the currently-running session to close (optimistically); it
    /// self-heals to the server's closed row on the next refetch.
    pub fn switch(&self, slug: String, open_id: Uuid, req: StartTimerRequest) {
        let now = Utc::now();
        let draft = OrgSession {
            slug: slug.clone(),
            session: draft_session(&req),
        };
        self.write.run(
            self.store,
            move |s| {
                // Close the outgoing row so the recent list doesn't show
                // two live clocks in the optimistic gap before reconcile.
                s.update_optimistic(Id::Real(open_id), |r| {
                    r.session.end_time = Some(now);
                });
                // The incoming session is the ticket we reconcile.
                s.insert_optimistic(draft).0
            },
            move || async move {
                crate::feeds::switch_timer(&slug, req)
                    .await
                    .map(|session| Some(OrgSession { slug, session }))
            },
        );
    }

    /// Retro-log a past, already-closed session (manual entry). It
    /// appears in the list instantly, then reconciles to the server's
    /// row (authoritative id + rate snapshot).
    pub fn log(&self, slug: String, req: timer_proto::LogSessionRequest) {
        let now = Utc::now();
        let draft = OrgSession {
            slug: slug.clone(),
            session: WorkSession {
                id: Uuid::nil(),
                org_id: req.org_id,
                user_id: req.user_id,
                project_id: req.project_id,
                project_path: req.project_path.clone(),
                description: req.description.clone(),
                start_time: req.start_time,
                end_time: Some(req.end_time),
                billable: req.billable_override.unwrap_or(false),
                rate_cents: 0,
                currency: String::new(),
                task_note_path: req.task_note_path.clone(),
                invoice_id: None,
                created_at: now,
                updated_at: now,
            },
        };
        run_create(self.write, self.store, draft, move |_| async move {
            crate::feeds::log_session(&slug, req)
                .await
                .map(|session| OrgSession { slug, session })
        });
    }

    /// Edit a session (description / billable), then reconcile.
    pub fn update(&self, slug: String, req: timer_proto::service::UpdateSessionRequest) {
        let id = req.id;
        let desc = req.description.clone();
        let billable = req.billable;
        self.write.run(
            self.store,
            move |s| {
                s.update_optimistic(Id::Real(id), move |r| {
                    if let Some(d) = desc {
                        r.session.description = d;
                    }
                    if let Some(b) = billable {
                        r.session.billable = b;
                    }
                })
            },
            move || async move {
                crate::feeds::update_session(&slug, req)
                    .await
                    .map(|session| Some(OrgSession { slug, session }))
            },
        );
    }

    /// Delete a session now; restored (and reported) on failure.
    pub fn delete(&self, slug: String, id: Uuid) {
        self.write.run(
            self.store,
            move |s| s.remove_optimistic(Id::Real(id)),
            move || async move { crate::feeds::delete_session(&slug, id).await.map(|()| None) },
        );
    }
}

// ── threads + messages (per-entity conversations) ───────────────────

/// Threads anchored to one project, as one [`AtomResult`]. `key` is
/// `(owning slug, project id)` — `None` while the project itself is
/// still resolving keeps the phase at `Loading`.
pub fn use_project_threads(
    key: Option<(String, Uuid)>,
) -> AtomResult<Vec<(Id<Uuid>, threads::Thread)>, String> {
    let store = use_thread_store();
    let key = use_memo(use_reactive!(|(key,)| key));
    use_store_list(store, move || {
        let k = key();
        async move {
            let (slug, pid) = k?;
            Some(crate::feeds::fetch_threads(&slug, "project", pid).await)
        }
    })
}

/// Messages of the selected thread. `key` is `(owning slug, thread
/// id)`; `None` (nothing selected) stays `Loading` — render its
/// `value().unwrap_or_default()` for an empty panel.
pub fn use_thread_messages(
    key: Option<(String, Uuid)>,
) -> AtomResult<Vec<(Id<Uuid>, threads::Message)>, String> {
    let store = use_message_store();
    let key = use_memo(use_reactive!(|(key,)| key));
    use_store_list(store, move || {
        let k = key();
        async move {
            let (slug, tid) = k?;
            Some(crate::feeds::fetch_thread_messages(&slug, tid).await)
        }
    })
}

/// Unsaved placeholder thread built from the create request.
pub fn draft_thread(req: &threads::CreateThreadRequest) -> threads::Thread {
    let now = Utc::now();
    threads::Thread {
        id: Uuid::nil(),
        org_id: req.org_id,
        entity_type: req.entity_type.clone(),
        entity_id: req.entity_id,
        title: req.title.clone(),
        kind: req.kind.clone(),
        resolved: false,
        resolved_by: None,
        source_kind: req.source_kind.clone(),
        source_ref: req.source_ref.clone(),
        source_url: req.source_url.clone(),
        created_by: req.created_by,
        created_at: now,
        updated_at: now,
    }
}

/// Unsaved placeholder message built from the post request.
pub fn draft_message(req: &threads::PostMessageRequest) -> threads::Message {
    let now = Utc::now();
    threads::Message {
        id: Uuid::nil(),
        thread_id: req.thread_id,
        org_id: req.org_id,
        author_id: req.author_id,
        author_label: req.author_label.clone(),
        body: req.body.clone(),
        reply_to: req.reply_to,
        source_kind: req.source_kind.clone(),
        external_id: req.external_id.clone(),
        original_text: req.original_text.clone(),
        source_url: req.source_url.clone(),
        posted_at: req.posted_at.unwrap_or(now),
        created_at: now,
        updated_at: now,
    }
}

/// Optimistic writes for conversations: open a thread / post a message.
#[derive(Clone, Copy)]
pub struct ThreadMutations {
    threads: ThreadStore,
    messages: MessageStore,
    thread_m: Mutation<String>,
    message_m: Mutation<String>,
}

pub fn use_thread_mutations() -> ThreadMutations {
    ThreadMutations {
        threads: use_thread_store(),
        messages: use_message_store(),
        thread_m: use_mutation(),
        message_m: use_mutation(),
    }
}

impl ThreadMutations {
    pub fn create_thread(&self, slug: String, req: threads::CreateThreadRequest) {
        let draft = draft_thread(&req);
        run_create(self.thread_m, self.threads, draft, move |_| async move {
            crate::feeds::create_thread(&slug, req).await
        });
    }

    pub fn post_message(&self, slug: String, req: threads::PostMessageRequest) {
        let draft = draft_message(&req);
        run_create(self.message_m, self.messages, draft, move |_| async move {
            crate::feeds::post_thread_message(&slug, req).await
        });
    }
}
