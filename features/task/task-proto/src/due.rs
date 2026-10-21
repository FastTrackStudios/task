//! One answer to "what is due now" — the queue Home's Today rail, the
//! Tasks list's Today section, the Work sidebar's count, the CLI's
//! `task task list --due` and the morning brief all read.
//!
//! PRODUCT.md's first principle is "never hide what is due", and it is
//! broken the moment two surfaces compute "today" differently: Home
//! counting what the Tasks list leaves out, the brief listing work the
//! sidebar doesn't count, a maintenance hour that exists in the browser
//! and not in the terminal. So the rule and the order live here, below
//! every UI, as pure functions on wire types: the server runs them
//! inside `TaskService::query` ([`crate::TaskListFilter::due_now`]) for
//! the CLI and the brief, and the web and phone call the same functions
//! against their stores.
//!
//! - **What is due** ([`is_due_now`]): open, and either in progress (the
//!   work in hand) or due on or before today. The hard `due` date only —
//!   `scheduled` is a soft plan, and recurring routines carry stale
//!   scheduled dates that would pin them here around the clock (the same
//!   reason [`crate::is_relevant`] lets only `due` override a gate).
//! - **Whose** ([`crate::is_mine`]): the Tasks list's scope — assigned to
//!   the signed-in person, or with nobody signed in, a next action.
//! - **In what order** ([`due_order`]): [`crate::relevance_rank`] first
//!   (in progress, then what the running mode is for, then the timer's
//!   project, then the rest), then oldest due date, then priority, then
//!   title — so a maintenance mode's errands lead the due work and
//!   overdue leads what is merely due today.
//!
//! The situation — which mode is running, which project the timer is
//! on, what device this is — is assembled once by
//! [`RelevanceContext::now`] and [`RelevanceContext::with_session`], so
//! every client passes the same focus for the same session
//! (`process.focus.everywhere`, `process.context.device`).

use std::cmp::Ordering;

use uuid::Uuid;

use crate::model::{Priority, Status, TaskInfo, is_due_on_or_before, status_is_open};
use crate::relevance::{RelevanceContext, relevance_rank};

/// What a running mode session brings forward: the context / tag names
/// of the steering kind its description names ("Maintenance hour" →
/// `errands`, `chores`, …), or nothing for an ordinary session. The
/// open session *is* the mode (`process.focus.mode`), so this is the
/// whole of "which hour am I in" as far as the list is concerned.
#[must_use]
pub fn mode_focus(description: &str) -> Vec<String> {
    scheduling_proto::BlockCategory::from_focus_label(description)
        .map(|c| c.surfaces().iter().map(|s| (*s).to_owned()).collect())
        .unwrap_or_default()
}

impl RelevanceContext {
    /// "Now" for a client: its local date and clock, where the person
    /// says they are, and what they're on (`phone`, `computer`). Empty
    /// strings count as unknown.
    // t[impl process.context.device]
    #[must_use]
    pub fn now(
        local_date: impl Into<String>,
        local_hhmm: Option<String>,
        location: Option<&str>,
        device: Option<&str>,
    ) -> Self {
        let known = |s: Option<&str>| {
            s.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            local_hhmm,
            local_date: Some(local_date.into()),
            location: known(location),
            device: known(device),
            active_project: None,
            focus: Vec::new(),
        }
    }

    /// Fold in the person's open timer session, if there is one: its
    /// project ranks first, and if it is a mode ([`mode_focus`]) the
    /// mode's focus opens its gates and leads the list.
    // t[impl process.focus.everywhere]
    #[must_use]
    pub fn with_session(mut self, description: &str, project: Option<Uuid>) -> Self {
        self.active_project = project;
        self.focus = mode_focus(description);
        self
    }
}

/// Whether the task is due now on `today` (`YYYY-MM-DD`): open, and in
/// progress or due on or before today. See the module docs.
#[must_use]
pub fn is_due_now(task: &TaskInfo, today: &str) -> bool {
    status_is_open(&task.status) && is_today_work(task, today)
}

/// The "Today" half of [`is_due_now`] without the open check — for a
/// list that has already set finished work aside (`task_ui`'s sections).
#[must_use]
pub fn is_today_work(task: &TaskInfo, today: &str) -> bool {
    Status::from_str(&task.status) == Some(Status::InProgress)
        || is_due_on_or_before(task.due.as_deref(), None, today)
}

fn priority_rank(p: &str) -> u8 {
    match Priority::from_str(p) {
        Some(Priority::Critical) => 0,
        Some(Priority::High) => 1,
        Some(Priority::Normal) | None => 2,
        Some(Priority::Low) => 3,
        Some(Priority::None) => 4,
    }
}

fn due_day(t: &TaskInfo) -> Option<&str> {
    t.due
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.get(..10).unwrap_or(s))
}

/// The due queue's order: [`relevance_rank`], then oldest due date
/// (undated — work in hand — after dated within a rank), then priority,
/// then title, then id so equal rows never swap between surfaces.
#[must_use]
pub fn due_order(a: &TaskInfo, b: &TaskInfo, ctx: &RelevanceContext) -> Ordering {
    relevance_rank(a, ctx)
        .cmp(&relevance_rank(b, ctx))
        .then_with(|| due_day(a).is_none().cmp(&due_day(b).is_none()))
        .then_with(|| due_day(a).cmp(&due_day(b)))
        .then_with(|| priority_rank(&a.priority).cmp(&priority_rank(&b.priority)))
        .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        .then_with(|| a.id.cmp(&b.id))
}

/// The due queue: `me`'s tasks ([`crate::is_mine`]) that are due now
/// ([`is_due_now`]) on `ctx.local_date`, in [`due_order`]. Empty when
/// the context has no date — nothing can be said to be due without one.
#[must_use]
pub fn due_queue<'a>(
    tasks: impl IntoIterator<Item = &'a TaskInfo>,
    me: Option<&str>,
    ctx: &RelevanceContext,
) -> Vec<TaskInfo> {
    let Some(today) = ctx.local_date.as_deref() else {
        return Vec::new();
    };
    let mut out: Vec<TaskInfo> = tasks
        .into_iter()
        .filter(|t| crate::is_mine(t, me) && is_due_now(t, today))
        .cloned()
        .collect();
    out.sort_by(|a, b| due_order(a, b, ctx));
    out
}

/// How many of a due queue are late — due before `today`.
#[must_use]
pub fn overdue_count(queue: &[TaskInfo], today: &str) -> usize {
    let today = today.get(..10).unwrap_or(today);
    queue
        .iter()
        .filter(|t| due_day(t).is_some_and(|d| d < today))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-10-20";

    fn task(title: &str, status: &str, due: Option<&str>, contexts: &[&str]) -> TaskInfo {
        let mut t = crate::capture(title);
        // Quick-add reads "today" / "tomorrow" in a title as a date;
        // these rows name themselves, so keep the words as given.
        t.title = title.to_owned();
        t.status = status.to_owned();
        t.due = due.map(str::to_owned);
        t.contexts = contexts.iter().map(ToString::to_string).collect();
        t
    }

    fn titles(q: &[TaskInfo]) -> Vec<&str> {
        q.iter().map(|t| t.title.as_str()).collect()
    }

    fn now() -> RelevanceContext {
        RelevanceContext::now(TODAY, Some("16:30".into()), None, Some("computer"))
    }

    #[test]
    fn due_is_open_and_in_hand_or_dated_today_or_before() {
        assert!(is_due_now(
            &task("a", "open", Some("2026-10-19"), &[]),
            TODAY
        ));
        assert!(is_due_now(&task("b", "open", Some(TODAY), &[]), TODAY));
        assert!(is_due_now(&task("c", "in-progress", None, &[]), TODAY));
        assert!(!is_due_now(
            &task("d", "open", Some("2026-10-21"), &[]),
            TODAY
        ));
        assert!(!is_due_now(&task("e", "open", None, &[]), TODAY));
        assert!(!is_due_now(
            &task("f", "done", Some("2026-10-01"), &[]),
            TODAY
        ));
        // Scheduled is a soft plan: it does not make a task due.
        let mut soft = task("g", "open", None, &[]);
        soft.scheduled = Some("2026-10-01".into());
        assert!(!is_due_now(&soft, TODAY));
    }

    #[test]
    fn the_queue_leads_with_work_in_hand_then_oldest_due() {
        let tasks = [
            task("Due today", "open", Some(TODAY), &[]),
            task("Way late", "open", Some("2026-10-01"), &[]),
            task("Comping", "in-progress", None, &[]),
            task("Tomorrow", "open", Some("2026-10-21"), &[]),
            task("Finished", "done", Some("2026-10-02"), &[]),
        ];
        let q = due_queue(&tasks, None, &now());
        assert_eq!(titles(&q), ["Comping", "Way late", "Due today"]);
        assert_eq!(overdue_count(&q, TODAY), 1);
    }

    #[test]
    fn a_running_maintenance_mode_leads_the_due_work_with_its_errands() {
        let tasks = [
            task("Late mix notes", "open", Some("2026-10-18"), &[]),
            task("Return the mic stands", "open", Some(TODAY), &["@errands"]),
        ];
        let plain = due_queue(&tasks, None, &now());
        assert_eq!(titles(&plain), ["Late mix notes", "Return the mic stands"]);
        let mode = now().with_session("Maintenance hour", None);
        assert!(mode.focus.iter().any(|f| f == "errands"));
        let led = due_queue(&tasks, None, &mode);
        assert_eq!(titles(&led), ["Return the mic stands", "Late mix notes"]);
    }

    #[test]
    fn an_ordinary_session_ranks_its_project_but_steers_nothing() {
        let project = Uuid::new_v4();
        let ctx = now().with_session("Comping vocals", Some(project));
        assert!(ctx.focus.is_empty());
        assert_eq!(ctx.active_project, Some(project));
        assert!(mode_focus("maintenance HOUR").contains(&"chores".to_owned()));
    }

    #[test]
    fn no_date_no_queue() {
        let tasks = [task("Late", "open", Some("2026-10-01"), &[])];
        assert!(due_queue(&tasks, None, &RelevanceContext::default()).is_empty());
    }

    #[test]
    fn unknown_device_and_place_are_left_empty() {
        let ctx = RelevanceContext::now(TODAY, None, Some(""), Some("  "));
        assert_eq!(ctx.location, None);
        assert_eq!(ctx.device, None);
    }

    #[test]
    fn whose_list_is_the_tasks_list_scope() {
        let mut theirs = task("Theirs", "open", Some(TODAY), &[]);
        theirs.workflow = Some(crate::model::WorkflowAttrs {
            assignees: crate::model::AgentRefList(vec![workflows_proto::AgentRef::Human {
                user_id: "sam".into(),
                display_name: None,
            }]),
            ..Default::default()
        });
        let q = due_queue(std::slice::from_ref(&theirs), Some("alice"), &now());
        assert!(
            q.is_empty(),
            "someone else's due work is not on Alice's list"
        );
        let q = due_queue(std::slice::from_ref(&theirs), Some("sam"), &now());
        assert_eq!(titles(&q), ["Theirs"]);
    }
}
