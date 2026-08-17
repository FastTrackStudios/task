//! Contextual relevance — which Active tasks deserve attention *right
//! now* — the relevancy-and-inbox doctrine.
//!
//! Rides on the GTD `contexts` field: a task carrying **gate
//! contexts** (`@morning`, `@home`, `@phone`, …) is visible only when
//! the caller's [`RelevanceContext`] satisfies at least one of them;
//! a task with no gate contexts is always relevant; a task due or
//! scheduled today (or overdue) always shows — deadlines trump gates.
//!
//! Pure functions on wire types, deliberately UI-free: the server
//! applies them inside `TaskService::query` (CLI path) and the web UI
//! calls the same functions client-side against its optimistic store.

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{TaskInfo, is_due_on_or_before, status_is_open};

/// The caller's situation, every field optional — an empty context
/// hides all gated tasks and keeps everything ungated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct RelevanceContext {
    /// Local wall-clock time as `HH:MM` (the *caller's* clock — the
    /// server never guesses a timezone). Drives the time-window
    /// contexts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub local_hhmm: Option<String>,
    /// Local date as `YYYY-MM-DD`, for the due/scheduled override.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub local_date: Option<String>,
    /// Where the user is (`home`, `studio`, `errands`, …) — matched
    /// against `@<location>` contexts, ASCII-case-insensitive.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub location: Option<String>,
    /// What they're on (`phone`, `computer`, …) — matched against
    /// `@<device>` contexts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub device: Option<String>,
    /// Project of the currently-running timer session; its tasks
    /// rank first.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_project: Option<Uuid>,
}

/// The built-in time-window contexts: `(@name, [(start, end)])` in
/// minutes-since-midnight, end-exclusive.
///
/// v1 fixed windows — personal windows move to the per-user prefs
/// entity later (see the plan).
const TIME_WINDOWS: &[(&str, &[(u16, u16)])] = &[
    ("morning", &[(5 * 60, 10 * 60)]),
    ("mealprep", &[(11 * 60, 13 * 60), (17 * 60, 19 * 60)]),
    ("evening", &[(20 * 60, 24 * 60)]),
];

/// Parse `HH:MM` to minutes-since-midnight. Garbage → `None`.
fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Strip the GTD `@` sigil and lowercase — `@Morning` → `morning`.
fn context_name(raw: &str) -> String {
    raw.trim().trim_start_matches('@').to_ascii_lowercase()
}

/// Whether `name` is a time-window context currently in-window.
/// Unknown names are not time contexts (returns `None`).
fn time_window_matches(name: &str, now_min: u16) -> Option<bool> {
    TIME_WINDOWS
        .iter()
        .find(|(w, _)| *w == name)
        .map(|(_, spans)| spans.iter().any(|&(a, b)| now_min >= a && now_min < b))
}

/// Does the context satisfy one gate context name?
fn gate_matches(name: &str, ctx: &RelevanceContext) -> bool {
    if let Some(now) = ctx.local_hhmm.as_deref().and_then(parse_hhmm) {
        if let Some(hit) = time_window_matches(name, now) {
            return hit;
        }
    } else if time_window_matches(name, 0).is_some() {
        // Time-window context but the caller supplied no clock —
        // treat as out-of-window (routines only show when asked
        // "what's relevant now", never in a timeless query).
        return false;
    }
    let eq = |v: &Option<String>| v.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(name));
    eq(&ctx.location) || eq(&ctx.device)
}

/// Whether the task is **relevant** under `ctx`. Assumes the caller
/// already scoped to Active tasks (see [`status_is_open`]) — done
/// tasks are neither relevant nor irrelevant, just filtered upstream.
#[must_use]
pub fn is_relevant(task: &TaskInfo, ctx: &RelevanceContext) -> bool {
    // HARD deadlines trump gates: a `due` date today or overdue
    // always shows. `scheduled` deliberately does NOT override —
    // it's a soft plan, and recurring routines carry stale
    // scheduled dates that would otherwise pin them visible
    // around the clock (the exact noise gating exists to cut).
    if let Some(today) = ctx.local_date.as_deref() {
        if is_due_on_or_before(task.due.as_deref(), None, today) {
            return true;
        }
    }
    let gates: Vec<String> = task.contexts.iter().map(|c| context_name(c)).collect();
    if gates.is_empty() {
        return true;
    }
    gates.iter().any(|g| gate_matches(g, ctx))
}

/// Ordering weight — smaller sorts first. The task being worked on
/// right now (in-progress — its timer is running) leads, then
/// active-timer project tasks, then due/overdue, then everything else
/// in the caller's existing order (stable sorts keep it).
#[must_use]
pub fn relevance_rank(task: &TaskInfo, ctx: &RelevanceContext) -> u8 {
    if crate::model::Status::from_str(&task.status) == Some(crate::model::Status::InProgress) {
        return 0;
    }
    if ctx.active_project.is_some() && task.project_id == ctx.active_project {
        return 1;
    }
    if let Some(today) = ctx.local_date.as_deref() {
        if is_due_on_or_before(task.due.as_deref(), task.scheduled.as_deref(), today) {
            return 2;
        }
    }
    3
}

/// The shared "Active + Relevant" pipeline: keep open tasks that are
/// relevant, stably ordered by [`relevance_rank`]. Both the server's
/// `query` filter and the web store's client-side view call this.
///
/// Unfiled tasks (see [`crate::filing`]) are **not** relevant — a row
/// that can't say what it belongs to isn't an answer to "what should
/// I do now", it's an answer to "what haven't I sorted". Callers that
/// want them (the triage surface) use [`partition_triage`] to take
/// them off the list first.
pub fn filter_relevant(tasks: &mut Vec<TaskInfo>, ctx: &RelevanceContext) {
    tasks
        .retain(|t| status_is_open(&t.status) && crate::filing::is_filed(t) && is_relevant(t, ctx));
    tasks.sort_by_key(|t| relevance_rank(t, ctx));
}

/// Split the unfiled open tasks off the front of a list, returning
/// them in triage order (oldest capture first — the thing you've been
/// ignoring longest gets sorted first). `tasks` keeps everything else.
///
/// This is the honest counterpart to [`filter_relevant`]'s exclusion:
/// nothing is dropped, it's routed. A surface that hides unfiled work
/// without offering somewhere to file it is just losing tasks.
pub fn partition_triage(tasks: &mut Vec<TaskInfo>) -> Vec<TaskInfo> {
    let mut triage: Vec<TaskInfo> = Vec::new();
    tasks.retain(|t| {
        if status_is_open(&t.status) && crate::filing::is_unfiled(t) {
            triage.push(t.clone());
            false
        } else {
            true
        }
    });
    triage.sort_by(|a, b| {
        a.date_created
            .cmp(&b.date_created)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    triage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(contexts: &[&str]) -> TaskInfo {
        let mut t = crate::capture("Test");
        t.contexts = contexts.iter().map(ToString::to_string).collect();
        t
    }

    fn at(hhmm: &str) -> RelevanceContext {
        RelevanceContext {
            local_hhmm: Some(hhmm.to_owned()),
            local_date: Some("2026-07-01".to_owned()),
            ..RelevanceContext::default()
        }
    }

    #[test]
    fn ungated_tasks_are_always_relevant() {
        assert!(is_relevant(&task(&[]), &at("14:00")));
        assert!(is_relevant(&task(&[]), &RelevanceContext::default()));
    }

    #[test]
    fn routine_contexts_gate_by_time_window() {
        let brush = task(&["@morning", "@evening"]);
        assert!(is_relevant(&brush, &at("07:30")));
        assert!(is_relevant(&brush, &at("21:00")));
        assert!(!is_relevant(&brush, &at("14:00")));
        // No clock in the context → routines hidden.
        assert!(!is_relevant(&brush, &RelevanceContext::default()));
    }

    #[test]
    fn mealprep_has_two_windows() {
        let lunch = task(&["@mealprep"]);
        assert!(is_relevant(&lunch, &at("12:00")));
        assert!(is_relevant(&lunch, &at("18:00")));
        assert!(!is_relevant(&lunch, &at("15:00")));
    }

    #[test]
    fn location_and_device_gates_match_case_insensitively() {
        let errand = task(&["@errands"]);
        let mut ctx = at("14:00");
        assert!(!is_relevant(&errand, &ctx));
        ctx.location = Some("Errands".to_owned());
        assert!(is_relevant(&errand, &ctx));

        let call = task(&["@phone"]);
        ctx.device = Some("phone".to_owned());
        assert!(is_relevant(&call, &ctx));
    }

    #[test]
    fn deadline_trumps_gates() {
        let mut overdue = task(&["@morning"]);
        overdue.due = Some("2026-06-30".to_owned());
        assert!(is_relevant(&overdue, &at("14:00")));
    }

    #[test]
    fn soft_scheduled_does_not_override_gates() {
        // Recurring routines carry stale `scheduled` dates; the gate
        // must still win outside its window.
        let mut habit = task(&["@morning"]);
        habit.scheduled = Some("2026-05-23".to_owned());
        assert!(!is_relevant(&habit, &at("14:00")));
        assert!(is_relevant(&habit, &at("07:30")));
    }

    #[test]
    fn pipeline_filters_done_and_ranks_active_project_first() {
        let pid = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut a = task(&[]);
        a.title = "other".into();
        a.project_id = Some(other);
        let mut b = task(&[]);
        b.title = "on the clock".into();
        b.project_id = Some(pid);
        let mut done = task(&[]);
        done.status = "done".into();
        done.project_id = Some(other);
        let mut hidden = task(&["@morning"]);
        hidden.title = "routine".into();

        let mut rows = vec![a, done, hidden, b];
        let ctx = RelevanceContext {
            active_project: Some(pid),
            ..at("14:00")
        };
        filter_relevant(&mut rows, &ctx);
        let titles: Vec<&str> = rows.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["on the clock", "other"]);
    }

    #[test]
    fn unfiled_tasks_are_not_relevant_they_are_triage() {
        let mut bare = task(&[]);
        bare.title = "Telemetry + Observability: Sentry".into();
        let mut filed = task(&[]);
        filed.title = "filed".into();
        filed.project_id = Some(Uuid::new_v4());

        let mut rows = vec![bare, filed];
        filter_relevant(&mut rows, &at("14:00"));
        let titles: Vec<&str> = rows.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["filed"], "a bare title is not an action");
    }

    #[test]
    fn partition_triage_routes_rather_than_drops() {
        let mut bare = task(&[]);
        bare.title = "Sentry".into();
        bare.date_created = Some("2026-01-01T00:00:00Z".parse().unwrap());
        let mut older = task(&[]);
        older.title = "older".into();
        older.date_created = Some("2025-01-01T00:00:00Z".parse().unwrap());
        let mut filed = task(&[]);
        filed.title = "filed".into();
        filed.project_id = Some(Uuid::new_v4());
        let mut closed = task(&[]);
        closed.title = "closed".into();
        closed.status = "done".into();

        let mut rows = vec![bare, filed, closed, older];
        let triage = partition_triage(&mut rows);

        let t: Vec<&str> = triage.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(t, vec!["older", "Sentry"], "oldest capture triaged first");
        let kept: Vec<&str> = rows.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            kept,
            vec!["filed", "closed"],
            "closed work is not triage, however bare"
        );
    }
}

/// Sort key picking a group's **next action**: anything in-progress
/// wins outright (you're already doing it), then soonest hard due
/// date (undated last), then priority, then title. Smaller is better.
///
/// Exposed because every surface that condenses also wants to order
/// the leftovers the same way — the web list's inline "N more in …"
/// expander sorts with this rather than re-deriving it.
#[must_use]
pub fn next_action_key(t: &TaskInfo) -> NextKey {
    fn priority_rank(p: &str) -> u8 {
        match crate::model::Priority::from_str(p) {
            Some(crate::model::Priority::Critical) => 0,
            Some(crate::model::Priority::High) => 1,
            Some(crate::model::Priority::Normal) | None => 2,
            Some(crate::model::Priority::Low) => 3,
            Some(crate::model::Priority::None) => 4,
        }
    }
    let in_progress =
        crate::model::Status::from_str(&t.status) == Some(crate::model::Status::InProgress);
    (
        !in_progress, // running work sorts first
        t.due.is_none(),
        t.due.clone().unwrap_or_default(),
        priority_rank(&t.priority),
        t.title.to_lowercase(),
    )
}

/// Sort key: (not-in-progress, undated, due, priority, title).
pub type NextKey = (bool, bool, String, u8, String);

/// Condense a Relevant view to the single **next action per anchor**
/// — a project with forty queued tasks contributes one row ("what
/// would I do here right now"), so task-dumping can't inflate the
/// list. Grouping is by [`crate::filing::Anchor`], so a triaged PRD
/// broken into a dozen subtasks also collapses to its next action,
/// not twelve rows.
///
/// `resolve` supplies each task's anchor. Pass
/// [`crate::filing::anchor`] for the pure structural answer; callers
/// holding a project list (the web store) pass a closure that first
/// resolves `projects:` wikilinks to a project id, so wikilink-only
/// membership condenses too. Returning `None` leaves the task
/// untouched — one-offs each keep their row.
pub fn condense_next_per_anchor<T, F>(tasks: &mut Vec<T>, resolve: F)
where
    T: std::borrow::Borrow<TaskInfo>,
    F: Fn(&TaskInfo) -> Option<crate::filing::Anchor>,
{
    use std::collections::HashMap;

    let mut winner: HashMap<crate::filing::Anchor, (usize, NextKey)> = HashMap::new();
    let mut anchors: Vec<Option<crate::filing::Anchor>> = Vec::with_capacity(tasks.len());
    for (i, t) in tasks.iter().enumerate() {
        let t = t.borrow();
        let a = resolve(t);
        anchors.push(a);
        let Some(a) = a else { continue };
        let k = next_action_key(t);
        match winner.get(&a) {
            Some((_, best)) if *best <= k => {}
            _ => {
                winner.insert(a, (i, k));
            }
        }
    }
    let keep: std::collections::HashSet<usize> = winner.into_values().map(|(i, _)| i).collect();
    let mut i = 0;
    tasks.retain(|_| {
        let keep_it = anchors[i].is_none() || keep.contains(&i);
        i += 1;
        keep_it
    });
}

/// [`condense_next_per_anchor`] over the structural anchor — the
/// backwards-compatible entry point for callers with no project list
/// to resolve wikilinks against (the server's `query`, the CLI).
pub fn condense_next_per_project<T: std::borrow::Borrow<TaskInfo>>(tasks: &mut Vec<T>) {
    condense_next_per_anchor(tasks, crate::filing::anchor);
}

#[cfg(test)]
mod condense_tests {
    use super::condense_next_per_project;
    use uuid::Uuid;

    fn task(
        title: &str,
        project: Option<Uuid>,
        due: Option<&str>,
        status: &str,
    ) -> crate::TaskInfo {
        let mut t = crate::capture(title);
        t.project_id = project;
        t.due = due.map(str::to_owned);
        t.status = status.into();
        t
    }

    #[test]
    fn one_row_per_project_soonest_due_wins() {
        let p = Uuid::new_v4();
        let q = Uuid::new_v4();
        let mut rows = vec![
            task("slice 3", Some(p), None, "open"),
            task("slice 2", Some(p), Some("2026-07-05"), "open"),
            task("standalone", None, None, "open"),
            task("wire the wiki", Some(q), None, "open"),
        ];
        condense_next_per_project(&mut rows);
        let titles: Vec<&str> = rows.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["slice 2", "standalone", "wire the wiki"]);
    }

    #[test]
    fn in_progress_beats_sooner_due() {
        let p = Uuid::new_v4();
        let mut rows = vec![
            task("due soon", Some(p), Some("2026-07-03"), "open"),
            task("on the clock", Some(p), None, "in-progress"),
        ];
        condense_next_per_project(&mut rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "on the clock");
    }

    #[test]
    fn subtasks_of_one_parent_condense_to_their_next_action() {
        // The case that motivated widening project → anchor: a
        // triaged PRD becomes N project-less subtasks, and N rows in
        // the list is exactly the noise condensation exists to cut.
        let parent = Uuid::new_v4();
        let sub = |title: &str, due: Option<&str>| {
            let mut t = task(title, None, due, "open");
            t.workflow = Some(crate::model::WorkflowAttrs {
                parent: Some(parent),
                ..Default::default()
            });
            t
        };
        let mut rows = vec![
            sub("attach identity to http", None),
            sub("attach identity to vox", Some("2026-07-05")),
            task("unrelated one-off", None, None, "open"),
        ];
        condense_next_per_project(&mut rows);
        let titles: Vec<&str> = rows.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["attach identity to vox", "unrelated one-off"]);
    }

    #[test]
    fn a_resolver_can_group_by_wikilink_only_membership() {
        // The web store's case: `projects: [[Task platform]]` with no
        // `projectId` yet. Structurally unanchored, but the caller
        // holds the project list and can say otherwise.
        let p = Uuid::new_v4();
        let mut a = task("slice 1", None, None, "open");
        a.projects.push("[[Task platform]]".into());
        let mut b = task("slice 2", None, Some("2026-07-05"), "open");
        b.projects.push("[[Task platform]]".into());

        let mut rows = vec![a, b];
        super::condense_next_per_anchor(&mut rows, |t| {
            t.projects
                .first()
                .map(|_| crate::filing::Anchor::Project(p))
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "slice 2");
    }
}
