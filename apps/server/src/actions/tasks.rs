//! Task actions: orientation and the task list (`task_context`,
//! `list_tasks`, `create_task`, `update_task`).
//!
//! Backed by the org's `task::TaskService` (plus the inbox and, when the
//! plugin is on, scheduling for the orientation counts). Per-org and
//! composite, so registered by hand (see the module docs of
//! [`super`]).

use architect::action::{ActionError, ActionList, ActionMeta, Effect};
use facet::Facet;
use serde_json::{Value, json};

use super::{CallContext, change, limit, meta, required, text, who};
use crate::mcp::{ToolFailure, backend_err, date_of, parse_assignee, parse_uuid, task_json};

/// `task_context`.
pub static CONTEXT: ActionMeta = meta(
    "TASKS_CONTEXT",
    "context",
    "Where am I",
    "Tasks",
    "Orient yourself: today's date and time, the org you're working in, and a count of what's \
     open (tasks due, events today, inbox items). Call this first when the user's request \
     depends on 'now' or 'what's on my plate'.",
);

/// `list_tasks`.
pub static LIST: ActionMeta = meta(
    "TASKS_LIST",
    "list",
    "List tasks",
    "Tasks",
    "List tasks. Defaults to open tasks only. Filter by status, or by 'due_on_or_before' (ISO \
     YYYY-MM-DD) to answer 'what's due today/this week'. Returns ids you must pass to \
     update_task.",
);

/// `create_task`.
pub static CREATE: ActionMeta = meta(
    "TASKS_CREATE",
    "create",
    "Create a task",
    "Tasks",
    "Create a task from natural language. The text is parsed the same way the app's quick-add \
     is: '#tag', '@context', '[[Project]]', '!high', and dates like 'tomorrow', 'next monday', \
     '2026-08-01' are extracted, and the remainder becomes the title. Explicit fields override \
     what the text implies.",
);

/// `update_task`.
pub static UPDATE: ActionMeta = meta(
    "TASKS_UPDATE",
    "update",
    "Update a task",
    "Tasks",
    "Change a task by id: mark it done, reschedule it, re-prioritize, or retitle. Only the \
     fields you pass change. Get ids from list_tasks.",
);

/// `task_context` takes nothing.
#[derive(Facet)]
pub struct ContextArgs {}

/// What `list_tasks` filters on.
#[derive(Facet)]
pub struct ListArgs {
    /// Exact status to match, e.g. 'open', 'in-progress', 'done'. Omit for all open tasks.
    pub status: Option<String>,
    /// ISO date (YYYY-MM-DD). Keeps tasks due or scheduled on/before it.
    pub due_on_or_before: Option<String>,
    /// Project UUID (from list_projects) — keeps tasks belonging to that project.
    pub project: Option<String>,
    /// Case-insensitive substring match on the title.
    pub query: Option<String>,
    /// Keep tasks made by this account — a created_by ref from a listed task ('bot:<uuid>', 'user:<uuid>'), a bare account uuid, 'me', or 'none' for tasks with no recorded creator.
    pub created_by: Option<String>,
    /// Keep tasks a personal assistant bot made on behalf of this person — 'me', a 'user:<uuid>' ref, a bare uuid, or 'none'.
    pub on_behalf_of: Option<String>,
    /// Keep tasks assigned to this principal — 'me', 'user:<uuid>' (a person or a bot account), 'agent:NAME', or 'none' for unassigned tasks.
    pub assignee: Option<String>,
    /// Max rows (default 50, max 200).
    pub limit: Option<u64>,
}

/// A task to make.
#[derive(Facet)]
pub struct CreateArgs {
    /// The task in natural language, e.g. 'Email Dana about the invoice tomorrow !high @work'.
    pub text: String,
    /// Override the due date (ISO YYYY-MM-DD).
    pub due: Option<String>,
    /// When you plan to work on it (ISO YYYY-MM-DD).
    pub scheduled: Option<String>,
    /// 'none' | 'low' | 'normal' | 'high' | 'critical'.
    pub priority: Option<String>,
}

/// The fields of a task to change; the absent ones stay.
#[derive(Facet)]
pub struct UpdateArgs {
    /// Task UUID from list_tasks.
    pub id: String,
    /// New status, e.g. 'open', 'in-progress', 'done'.
    pub status: Option<String>,
    /// New title.
    pub title: Option<String>,
    /// New due date (ISO YYYY-MM-DD); pass an empty string to clear.
    pub due: Option<String>,
    /// New scheduled date (ISO YYYY-MM-DD); empty string clears.
    pub scheduled: Option<String>,
    /// 'none' | 'low' | 'normal' | 'high' | 'critical'.
    pub priority: Option<String>,
    /// Replace who owns the task: 'agent:NAME' or 'human:USER_ID' entries (bare names mean agent:); a human must be an account in this org. Empty array unclaims. Prefer claim_task to take a task for yourself.
    pub assignees: Option<Vec<String>>,
}

/// Put the task actions on `list`.
///
/// # Errors
///
/// [`ActionError::Duplicate`] when one is already there.
pub fn register(list: &ActionList) -> Result<(), ActionError> {
    super::register(list, &CONTEXT, Effect::ReadOnly, context)?;
    super::register(list, &LIST, Effect::ReadOnly, list_tasks)?;
    super::register(list, &CREATE, Effect::Mutating, create)?;
    super::register(list, &UPDATE, Effect::Mutating, update)
}

fn context(ctx: &CallContext, _: ContextArgs) -> Result<Value, ToolFailure> {
    use inbox_proto::Inbox as _;
    #[cfg(feature = "plugin-scheduling")]
    use scheduling_proto::CalendarEvents as _;
    use task::TaskService as _;
    let org = &ctx.org;
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let open = org
        .tasks
        .query(task::TaskListFilter {
            open_only: true,
            ..Default::default()
        })
        .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
    let due_today = open
        .iter()
        .filter(|t| {
            t.due
                .as_deref()
                .is_some_and(|d| date_of(d) <= today.as_str())
                || t.scheduled
                    .as_deref()
                    .is_some_and(|d| date_of(d) <= today.as_str())
        })
        .count();
    // Scheduling is a plugin twice over: compiled out under
    // --no-default-features (cfg) and toggleable per org at runtime
    // (PluginSet) — a core orientation call must not touch a backend
    // that is off either way.
    #[cfg(feature = "plugin-scheduling")]
    let events_today = if org.plugins.contains("scheduling") {
        org.scheduling
            .list_events()
            .map(|evs| {
                evs.iter()
                    .filter(|e| e.tombstone.is_none() && date_of(&e.start) == today)
                    .count()
            })
            .unwrap_or(0)
    } else {
        0
    };
    #[cfg(not(feature = "plugin-scheduling"))]
    let events_today = 0;
    let inbox_open = org
        .inbox
        .review_queue(today.clone())
        .map(|q| q.len())
        .unwrap_or(0);
    let creator = ctx.creator.as_ref();
    Ok(json!({
        "org": org.slug,
        "now": now.to_rfc3339(),
        "today": today,
        "weekday": now.format("%A").to_string(),
        "open_tasks": open.len(),
        "tasks_due_or_scheduled_today": due_today,
        "events_today": events_today,
        "inbox_awaiting_review": inbox_open,
        // The caller's own creator ref — what `created_by` filters take
        // to mean "what I made".
        "you": creator.map(|c| json!({
            "created_by": c.r#ref,
            "name": c.name,
            // A personal assistant's person: what it makes is stamped on
            // behalf of them.
            "acting_for": c.on_behalf_of.as_ref().map(|p| json!({ "ref": p.r#ref, "name": p.name })),
        })),
    }))
}

fn list_tasks(ctx: &CallContext, args: ListArgs) -> Result<Value, ToolFailure> {
    use task::TaskService as _;
    let status = text(args.status);
    let project = match text(args.project) {
        Some(p) => Some(parse_uuid(&p, "project")?),
        None => None,
    };
    let filter = task::TaskListFilter {
        // An explicit status wins; without one we show what's
        // actionable, which is what "my tasks" means.
        open_only: status.is_none(),
        status,
        project,
        due_on_or_before: text(args.due_on_or_before),
        created_by: who(args.created_by, ctx),
        on_behalf_of: who(args.on_behalf_of, ctx),
        assignee: who(args.assignee, ctx),
        ..Default::default()
    };
    let query = text(args.query).map(|q| q.to_lowercase());
    let rows = ctx
        .org
        .tasks
        .query(filter)
        .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
    let out: Vec<Value> = rows
        .iter()
        .filter(|t| {
            query
                .as_ref()
                .is_none_or(|q| t.title.to_lowercase().contains(q))
        })
        .take(limit(args.limit))
        .map(task_json)
        .collect();
    Ok(json!({ "count": out.len(), "tasks": out }))
}

fn create(ctx: &CallContext, args: CreateArgs) -> Result<Value, ToolFailure> {
    use task::TaskService as _;
    let text_arg = required(args.text, "text")?;
    let mut draft = task::capture(&text_arg);
    if let Some(due) = text(args.due) {
        draft.due = Some(due);
    }
    if let Some(scheduled) = text(args.scheduled) {
        draft.scheduled = Some(scheduled);
    }
    if let Some(priority) = text(args.priority) {
        draft.priority = priority;
    }
    crate::attribution::attribute_task(&mut draft, ctx.creator.as_ref());
    let created = ctx
        .org
        .tasks
        .create(draft)
        .map_err(|e| backend_err("task", &text_arg, &e))?;
    Ok(task_json(&created))
}

fn update(ctx: &CallContext, args: UpdateArgs) -> Result<Value, ToolFailure> {
    use task::TaskService as _;
    let org = &ctx.org;
    let id = required(args.id, "id")?;
    let uuid = id
        .parse::<uuid::Uuid>()
        .map_err(|_| ToolFailure::Message(format!("`{id}` is not a task id")))?;
    let mut current = org
        .tasks
        .get(uuid)
        .map_err(|e| backend_err("task", &id, &e))?;
    if let Some(status) = text(args.status) {
        current.status = status;
    }
    if let Some(title) = text(args.title) {
        current.title = title;
    }
    if let Some(priority) = text(args.priority) {
        current.priority = priority;
    }
    if let Some(due) = change(args.due) {
        current.due = due;
    }
    if let Some(scheduled) = change(args.scheduled) {
        current.scheduled = scheduled;
    }
    if let Some(refs) = args.assignees {
        let parsed: Result<Vec<_>, ToolFailure> = refs
            .iter()
            .filter_map(|r| text(Some(r.clone())))
            .map(|r| parse_assignee(org, &r))
            .collect();
        current
            .workflow
            .get_or_insert_with(Default::default)
            .assignees = task::model::AgentRefList(parsed?);
    }
    let saved = org
        .tasks
        .update(current)
        .map_err(|e| backend_err("task", &id, &e))?;
    Ok(task_json(&saved))
}
