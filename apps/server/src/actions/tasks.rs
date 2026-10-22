//! Tasks: orient, list, create, update.

use std::sync::Arc;

use architect::action::{ActionError, ActionList, ActionMeta, Effect};
use facet::Facet;
use task::TaskService as _;

use super::{Ctx, meta, note_writer, present, register_blocking, who};
use crate::mcp::{backend_err_text, clamp_limit, parse_assignee_text};

pub(super) static CONTEXT: ActionMeta = meta(
    "TASK_CONTEXT",
    "task_context",
    "Task context",
    "tasks",
    "Orient yourself: today's date and time, the org you're working in, and a count of \
     what's open (tasks due, events today, inbox items). Call this first when the user's \
     request depends on 'now' or 'what's on my plate'.",
);

pub(super) static LIST: ActionMeta = meta(
    "TASK_LIST",
    "list_tasks",
    "List tasks",
    "tasks",
    "List tasks. Defaults to open tasks only. Filter by status, or by 'due_on_or_before' \
     (ISO YYYY-MM-DD) to answer 'what's due today/this week'. Returns ids you must pass to \
     update_task.",
);

pub(super) static CREATE: ActionMeta = meta(
    "TASK_CREATE",
    "create_task",
    "Create task",
    "tasks",
    "Create a task from natural language. The text is parsed the same way the app's \
     quick-add is: '#tag', '@context', '[[Project]]', '!high', and dates like 'tomorrow', \
     'next monday', '2026-08-01' are extracted, and the remainder becomes the title. \
     Explicit fields override what the text implies.",
);

pub(super) static UPDATE: ActionMeta = meta(
    "TASK_UPDATE",
    "update_task",
    "Update task",
    "tasks",
    "Change a task by id: mark it done, reschedule it, re-prioritize, or retitle. Only the \
     fields you pass change. Get ids from list_tasks.",
);

pub(super) fn register(list: &ActionList, ctx: Option<&Arc<Ctx>>) -> Result<(), ActionError> {
    register_blocking(list, &CONTEXT, Effect::ReadOnly, ctx, |ctx, ()| {
        Ok(context(ctx, chrono::Local::now()))
    })?;
    register_blocking(list, &LIST, Effect::ReadOnly, ctx, list_tasks)?;
    register_blocking(list, &CREATE, Effect::Mutating, ctx, create_task)?;
    register_blocking(list, &UPDATE, Effect::Mutating, ctx, update_task)
}

// ── task_context ─────────────────────────────────────────────────

/// Where and when the caller is, and what's open.
#[derive(Facet, Debug)]
pub struct Context {
    org: String,
    now: String,
    today: String,
    weekday: String,
    open_tasks: usize,
    tasks_due_or_scheduled_today: usize,
    events_today: usize,
    inbox_awaiting_review: usize,
    /// The caller's own creator ref — what `created_by` filters take
    /// to mean "what I made".
    you: Option<You>,
}

#[derive(Facet, Debug)]
pub struct You {
    created_by: String,
    name: Option<String>,
    /// A personal assistant's person: what it makes is stamped on
    /// behalf of them.
    acting_for: Option<ActingFor>,
}

#[derive(Facet, Debug)]
pub struct ActingFor {
    #[facet(rename = "ref")]
    reference: String,
    name: Option<String>,
}

/// The counts degrade to zero when a backend can't answer: orientation
/// must not fail because one lane is unwell.
fn context(ctx: &Ctx, now: chrono::DateTime<chrono::Local>) -> Context {
    let org = &ctx.org;
    let today = now.format("%Y-%m-%d").to_string();
    let open = open_tasks(org);
    Context {
        org: org.slug.clone(),
        now: now.to_rfc3339(),
        weekday: now.format("%A").to_string(),
        open_tasks: open.len(),
        tasks_due_or_scheduled_today: open.iter().filter(|t| due_by(t, &today)).count(),
        events_today: events_on(org, &today),
        inbox_awaiting_review: inbox_awaiting(org, &today),
        you: ctx.creator.as_ref().map(|c| You {
            created_by: c.r#ref.clone(),
            name: c.name.clone(),
            acting_for: c.on_behalf_of.as_ref().map(|p| ActingFor {
                reference: p.r#ref.clone(),
                name: p.name.clone(),
            }),
        }),
        today,
    }
}

fn open_tasks(org: &crate::OrgAppState) -> Vec<task::TaskInfo> {
    org.tasks
        .query(task::TaskListFilter {
            open_only: true,
            ..Default::default()
        })
        .unwrap_or_default()
}

/// Whether `t` is due or scheduled on or before `today`.
fn due_by(t: &task::TaskInfo, today: &str) -> bool {
    [t.due.as_deref(), t.scheduled.as_deref()]
        .into_iter()
        .flatten()
        .any(|d| date_of(d) <= today)
}

/// The date part of an RFC-3339 or ISO timestamp.
fn date_of(stamp: &str) -> &str {
    stamp.split('T').next().unwrap_or(stamp)
}

/// Scheduling is a plugin twice over: compiled out under
/// `--no-default-features` (cfg) and toggleable per org at runtime
/// (`PluginSet`) — a core orientation call must not touch a backend
/// that is off either way.
#[cfg(feature = "plugin-scheduling")]
fn events_on(org: &crate::OrgAppState, day: &str) -> usize {
    use scheduling_proto::CalendarEvents as _;
    if !org.plugins.contains("scheduling") {
        return 0;
    }
    org.scheduling.list_events().map_or(0, |evs| {
        evs.iter()
            .filter(|e| e.tombstone.is_none() && date_of(&e.start) == day)
            .count()
    })
}

#[cfg(not(feature = "plugin-scheduling"))]
fn events_on(_org: &crate::OrgAppState, _day: &str) -> usize {
    0
}

fn inbox_awaiting(org: &crate::OrgAppState, today: &str) -> usize {
    use inbox_proto::Inbox as _;
    org.inbox
        .review_queue(today.to_owned())
        .map_or(0, |q| q.len())
}

// ── list_tasks ───────────────────────────────────────────────────

#[derive(Facet, Debug)]
pub struct ListTasks {
    /// Exact status to match, e.g. 'open', 'in-progress', 'done'. Omit for all open tasks.
    status: Option<String>,
    /// ISO date (YYYY-MM-DD). Keeps tasks due or scheduled on/before it.
    due_on_or_before: Option<String>,
    /// Project UUID (from list_projects) — keeps tasks belonging to that project.
    project: Option<String>,
    /// Case-insensitive substring match on the title.
    query: Option<String>,
    /// Keep tasks made by this account — a created_by ref from a listed task ('bot:<uuid>', 'user:<uuid>'), a bare account uuid, 'me', or 'none' for tasks with no recorded creator.
    created_by: Option<String>,
    /// Keep tasks a personal assistant bot made on behalf of this person — 'me', a 'user:<uuid>' ref, a bare uuid, or 'none'.
    on_behalf_of: Option<String>,
    /// Keep tasks assigned to this principal — 'me', 'user:<uuid>' (a person or a bot account), 'agent:NAME', or 'none' for unassigned tasks.
    assignee: Option<String>,
    /// Max rows (default 50, max 200).
    limit: Option<u64>,
}

#[derive(Facet, Debug)]
pub struct TaskList {
    count: usize,
    tasks: Vec<TaskView>,
}

fn list_tasks(ctx: &Ctx, args: ListTasks) -> Result<TaskList, String> {
    let creator = ctx.creator.as_ref();
    let status = present(args.status);
    let project = present(args.project)
        .map(|p| parse_uuid(&p, "project"))
        .transpose()?;
    let filter = task::TaskListFilter {
        // An explicit status wins; without one we show what's
        // actionable, which is what "my tasks" means.
        open_only: status.is_none(),
        status,
        project,
        due_on_or_before: present(args.due_on_or_before),
        created_by: who(args.created_by, creator),
        on_behalf_of: who(args.on_behalf_of, creator),
        assignee: who(args.assignee, creator),
        ..Default::default()
    };
    let query = present(args.query).map(|q| q.to_lowercase());
    let rows = ctx.org.tasks.query(filter).map_err(|e| format!("{e:?}"))?;
    let tasks: Vec<TaskView> = rows
        .iter()
        .filter(|t| title_matches(t, query.as_deref()))
        .take(clamp_limit(args.limit))
        .map(TaskView::from)
        .collect();
    Ok(TaskList {
        count: tasks.len(),
        tasks,
    })
}

fn title_matches(t: &task::TaskInfo, query: Option<&str>) -> bool {
    query.is_none_or(|q| t.title.to_lowercase().contains(q))
}

fn parse_uuid(value: &str, what: &str) -> Result<uuid::Uuid, String> {
    value
        .parse()
        .map_err(|_| format!("`{value}` is not a {what} id (expected a UUID)"))
}

// ── create_task ──────────────────────────────────────────────────

#[derive(Facet, Debug)]
pub struct CreateTask {
    /// The task in natural language, e.g. 'Email Dana about the invoice tomorrow !high @work'.
    text: String,
    /// Override the due date (ISO YYYY-MM-DD).
    due: Option<String>,
    /// When you plan to work on it (ISO YYYY-MM-DD).
    scheduled: Option<String>,
    /// 'none' | 'low' | 'normal' | 'high' | 'critical'.
    priority: Option<String>,
}

fn create_task(ctx: &Ctx, args: CreateTask) -> Result<TaskView, String> {
    let text = present(Some(args.text)).ok_or("`text` is required")?;
    note_writer(ctx, "task:new".to_owned());
    let mut draft = task::capture(&text);
    if let Some(due) = present(args.due) {
        draft.due = Some(due);
    }
    if let Some(scheduled) = present(args.scheduled) {
        draft.scheduled = Some(scheduled);
    }
    if let Some(priority) = present(args.priority) {
        draft.priority = priority;
    }
    crate::attribution::attribute_task(&mut draft, ctx.creator.as_ref());
    let created = ctx
        .org
        .tasks
        .create(draft)
        .map_err(|e| backend_err_text("task", &text, &e))?;
    Ok(TaskView::from(&created))
}

// ── update_task ──────────────────────────────────────────────────

#[derive(Facet, Debug)]
pub struct UpdateTask {
    /// Task UUID from list_tasks.
    id: String,
    /// New status, e.g. 'open', 'in-progress', 'done'.
    status: Option<String>,
    /// New title.
    title: Option<String>,
    /// New due date (ISO YYYY-MM-DD); pass an empty string to clear.
    due: Option<String>,
    /// New scheduled date (ISO YYYY-MM-DD); empty string clears.
    scheduled: Option<String>,
    /// 'none' | 'low' | 'normal' | 'high' | 'critical'.
    priority: Option<String>,
    /// Replace who owns the task: 'agent:NAME' or 'human:USER_ID' entries (bare names mean agent:); a human must be an account in this org. Empty array unclaims. Prefer claim_task to take a task for yourself.
    assignees: Option<Vec<String>>,
}

fn update_task(ctx: &Ctx, args: UpdateTask) -> Result<TaskView, String> {
    let id = present(Some(args.id)).ok_or("`id` is required")?;
    note_writer(ctx, format!("task:{id}"));
    let uuid: uuid::Uuid = id.parse().map_err(|_| format!("`{id}` is not a task id"))?;
    let org = &ctx.org;
    let mut current = org
        .tasks
        .get(uuid)
        .map_err(|e| backend_err_text("task", &id, &e))?;
    if let Some(status) = present(args.status) {
        current.status = status;
    }
    if let Some(title) = present(args.title) {
        current.title = title;
    }
    if let Some(priority) = present(args.priority) {
        current.priority = priority;
    }
    if let Some(due) = patch(args.due) {
        current.due = due;
    }
    if let Some(scheduled) = patch(args.scheduled) {
        current.scheduled = scheduled;
    }
    if let Some(refs) = args.assignees {
        current
            .workflow
            .get_or_insert_with(Default::default)
            .assignees = task::model::AgentRefList(assignees(org, &refs)?);
    }
    let saved = org
        .tasks
        .update(current)
        .map_err(|e| backend_err_text("task", &id, &e))?;
    Ok(TaskView::from(&saved))
}

/// A clearable field: absent leaves it alone (`None`), blank clears it
/// (`Some(None)`), anything else sets it.
fn patch(value: Option<String>) -> Option<Option<String>> {
    value.map(|v| present(Some(v)))
}

/// Assignee refs, each a well-formed ref and (for a person) an account
/// in `org`; blank entries are dropped.
fn assignees(
    org: &crate::OrgAppState,
    refs: &[String],
) -> Result<Vec<task::workflows_proto::AgentRef>, String> {
    refs.iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| parse_assignee_text(org, r.trim()))
        .collect()
}

// ── The task as tools return it ──────────────────────────────────

/// One task as a tool returns it — also what `mcp.rs`'s remaining task
/// tools (`claim_task`, `file_task`, …) answer with.
#[derive(Facet, Debug)]
pub struct TaskView {
    id: String,
    title: String,
    status: String,
    priority: String,
    due: Option<String>,
    scheduled: Option<String>,
    tags: Vec<String>,
    contexts: Vec<String>,
    projects: Vec<String>,
    // Filing, spelled out: without these an agent can't tell a task
    // that belongs somewhere from one that doesn't, and "which of my
    // tasks need sorting" is unanswerable.
    project_id: Option<String>,
    parent: Option<String>,
    workstream: Option<String>,
    milestone_id: Option<String>,
    filed: bool,
    assignees: Option<Vec<String>>,
    path: String,
    /// Who made it (`user:<uuid>` / `bot:<uuid>`), and their name.
    created_by: Option<String>,
    created_by_name: Option<String>,
    /// Whom a personal assistant made it for (`user:<uuid>`), if one did.
    on_behalf_of: Option<String>,
    on_behalf_of_name: Option<String>,
}

impl From<&task::TaskInfo> for TaskView {
    fn from(t: &task::TaskInfo) -> Self {
        let workflow = t.workflow.as_ref();
        Self {
            id: t.id.to_string(),
            title: t.title.clone(),
            status: t.status.clone(),
            priority: t.priority.clone(),
            due: t.due.clone(),
            scheduled: t.scheduled.clone(),
            tags: t.tags.0.clone(),
            contexts: t.contexts.0.clone(),
            projects: t.projects.0.clone(),
            project_id: t.project_id.map(|id| id.to_string()),
            parent: workflow.and_then(|w| w.parent).map(|id| id.to_string()),
            workstream: workflow.and_then(|w| w.workstream).map(|id| id.to_string()),
            milestone_id: t.milestone_id.map(|id| id.to_string()),
            filed: task::is_filed(t),
            assignees: workflow.map(|w| w.assignees.0.iter().map(|a| a.short_label()).collect()),
            path: t.path.clone(),
            created_by: t.created_by.clone(),
            created_by_name: t.created_by_name.clone(),
            on_behalf_of: t.on_behalf_of.clone(),
            on_behalf_of_name: t.on_behalf_of_name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_separates_absent_from_cleared() {
        assert_eq!(patch(None), None);
        assert_eq!(patch(Some("  ".into())), Some(None));
        assert_eq!(
            patch(Some(" 2026-08-01 ".into())),
            Some(Some("2026-08-01".into()))
        );
    }

    #[test]
    fn due_by_reads_either_date_and_either_stamp_form() {
        let mut t = task::capture("x");
        t.due = None;
        t.scheduled = None;
        assert!(!due_by(&t, "2026-08-01"));
        t.scheduled = Some("2026-08-01T09:00:00Z".into());
        assert!(due_by(&t, "2026-08-01"));
        t.scheduled = None;
        t.due = Some("2026-08-02".into());
        assert!(!due_by(&t, "2026-08-01"));
    }
}
