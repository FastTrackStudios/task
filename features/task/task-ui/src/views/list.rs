//! Grouped list view — **Today**, **Upcoming**, **Anytime**, **Done**.
//!
//! The shape a person plans a day in: what needs doing now, what is
//! coming and when, and everything else by project. Every task the
//! page passes in is on screen — nothing folds behind an expander — so
//! the list and the counts above it always agree.
//!
//! - **Today** — overdue, due today, and anything in progress whatever
//!   its date. Overdue keeps its red date pill; in progress leads.
//! - **Upcoming** — due after today, soonest first.
//! - **Anytime** — undated, under one sub-heading per project (or
//!   whatever the row belongs to). The headings appear only when there
//!   is more than one group: inside a project's own sidebar they would
//!   all say the same thing.
//! - **Done** — collapsed until asked for.

use std::collections::BTreeMap;

use architect_ui::lucide_dioxus::ChevronRight;
use chrono::NaiveDate;
use dioxus::prelude::*;
use uuid::Uuid;

use task_proto::{Priority, Status};

use crate::display::TaskDisplay;
use crate::{TaskInfo, TaskMutation};

use super::row::{AnchorChip, TaskRow, strip_wikilink};

#[derive(Props, Clone, PartialEq)]
pub struct TaskListProps {
    pub tasks: Vec<TaskInfo>,
    /// `(task id, chip)` for rows whose belonging isn't a project —
    /// subtasks, workstream members, milestone work. Resolved by the
    /// page layer, which holds the stores those names live in.
    #[props(default)]
    pub anchors: Vec<(Uuid, AnchorChip)>,
    /// `(task id, org label)` — filled only when the list spans more
    /// than one org, so a row says whose it is.
    #[props(default)]
    pub org_of: Vec<(Uuid, String)>,
    pub on_toggle: EventHandler<Uuid>,
    pub on_open: EventHandler<Uuid>,
    /// Start / stop work on a task.
    pub on_track: EventHandler<Uuid>,
    pub on_event: EventHandler<TaskMutation>,
}

/// Everything a row reports back, bundled so the list threads one
/// value instead of four.
#[derive(Clone, Copy, PartialEq)]
struct Handlers {
    on_toggle: EventHandler<Uuid>,
    on_open: EventHandler<Uuid>,
    on_track: EventHandler<Uuid>,
    on_event: EventHandler<TaskMutation>,
}

/// The list's sections, public so anything that counts them (the
/// sidebar's smart lists) uses the same rule as the list itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TaskSection {
    Today,
    Upcoming,
    Anytime,
    Done,
}

use TaskSection as Group;

impl TaskSection {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Upcoming => "Upcoming",
            Self::Anytime => "Anytime",
            Self::Done => "Done",
        }
    }

    /// The section's element id on the Tasks page — what a link to
    /// "Upcoming" scrolls to.
    #[must_use]
    pub fn anchor(self) -> &'static str {
        match self {
            Self::Today => "tasks-today",
            Self::Upcoming => "tasks-upcoming",
            Self::Anytime => "tasks-anytime",
            Self::Done => "tasks-done",
        }
    }
}

const ORDER: &[Group] = &[Group::Today, Group::Upcoming, Group::Anytime, Group::Done];

/// Which section a task belongs in on `today`.
#[must_use]
pub fn classify(t: &TaskInfo, today: NaiveDate) -> TaskSection {
    if t.is_done() {
        return Group::Done;
    }
    if t.status_enum() == Status::InProgress {
        return Group::Today;
    }
    match t.due_date() {
        Some(d) if d <= today => Group::Today,
        Some(_) => Group::Upcoming,
        None => Group::Anytime,
    }
}

fn priority_rank(p: Priority) -> u8 {
    match p {
        Priority::Critical => 0,
        Priority::High => 1,
        Priority::Normal => 2,
        Priority::Low => 3,
        Priority::None => 4,
    }
}

/// Dated work first, oldest date first (so overdue leads, then today,
/// then soonest upcoming); undated work — in-progress among it — after;
/// then priority, then title. Overdue is what a person must not miss,
/// so nothing sorts above it.
fn sort_bucket(bucket: &mut [TaskInfo]) {
    bucket.sort_by(|a, b| {
        let undated = |t: &TaskInfo| t.due_date().is_none();
        undated(a)
            .cmp(&undated(b))
            .then_with(|| a.due_date().cmp(&b.due_date()))
            .then_with(|| {
                let idle = |t: &TaskInfo| t.status_enum() != Status::InProgress;
                idle(a).cmp(&idle(b))
            })
            .then_with(|| priority_rank(a.priority_enum()).cmp(&priority_rank(b.priority_enum())))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// Today's overdue part, oldest first — the band at the top of Today.
pub(crate) fn overdue_of(today_items: &[TaskInfo], today: NaiveDate) -> Vec<TaskInfo> {
    today_items
        .iter()
        .filter(|t| t.due_date().is_some_and(|d| d < today))
        .cloned()
        .collect()
}

/// Move every overdue task to `day`, one write each.
fn reschedule(overdue: &[TaskInfo], day: NaiveDate, on_event: EventHandler<TaskMutation>) {
    for t in overdue {
        let mut next = t.clone();
        next.due = Some(day.to_string());
        on_event.call(TaskMutation::Update { task: next });
    }
}

/// The Anytime sub-heading a task files under: its project, else what
/// it belongs to (a parent, a workstream), else `None` — the
/// unheaded group, listed last.
fn home_of(t: &TaskInfo, anchors: &[(Uuid, AnchorChip)]) -> Option<String> {
    t.projects
        .0
        .first()
        .map(|p| strip_wikilink(p))
        .filter(|p| !p.is_empty())
        .or_else(|| {
            anchors
                .iter()
                .find(|(id, _)| *id == t.id)
                .map(|(_, chip)| chip.label.clone())
        })
}

/// Anytime, split by [`home_of`]: named groups alphabetically, then
/// the unnamed one.
pub(crate) fn by_home(
    tasks: Vec<TaskInfo>,
    anchors: &[(Uuid, AnchorChip)],
) -> Vec<(Option<String>, Vec<TaskInfo>)> {
    let mut named: BTreeMap<String, (String, Vec<TaskInfo>)> = BTreeMap::new();
    let mut loose = Vec::new();
    for t in tasks {
        match home_of(&t, anchors) {
            Some(name) => named
                .entry(name.to_lowercase())
                .or_insert_with(|| (name, Vec::new()))
                .1
                .push(t),
            None => loose.push(t),
        }
    }
    let mut out: Vec<(Option<String>, Vec<TaskInfo>)> = named
        .into_values()
        .map(|(name, items)| (Some(name), items))
        .collect();
    if !loose.is_empty() {
        out.push((None, loose));
    }
    out
}

#[component]
pub fn TaskList(props: TaskListProps) -> Element {
    let today = chrono::Local::now().date_naive();
    let mut groups: Vec<(Group, Vec<TaskInfo>)> = ORDER.iter().map(|g| (*g, Vec::new())).collect();
    for t in &props.tasks {
        let g = classify(t, today);
        if let Some(slot) = groups.iter_mut().find(|(k, _)| *k == g) {
            slot.1.push(t.clone());
        }
    }
    for (_, bucket) in &mut groups {
        sort_bucket(bucket);
    }
    let groups: Vec<(Group, Vec<TaskInfo>)> = groups
        .into_iter()
        .filter(|(_, items)| !items.is_empty())
        .collect();
    let handlers = Handlers {
        on_toggle: props.on_toggle,
        on_open: props.on_open,
        on_track: props.on_track,
        on_event: props.on_event,
    };
    let anchors = props.anchors.clone();
    let org_of = props.org_of.clone();
    let rows = move |items: Vec<TaskInfo>| {
        rsx! { Rows { items, anchors: anchors.clone(), org_of: org_of.clone(), handlers } }
    };

    rsx! {
        div { class: "flex min-w-0 flex-col gap-5",
            if groups.is_empty() {
                p { class: "px-1 py-6 text-sm text-muted-foreground", "Nothing to do. Add a task above." }
            }
            for (group, items) in groups.into_iter() {
                Section {
                    key: "{group.label()}",
                    id: group.anchor(),
                    label: group.label(),
                    count: items.len(),
                    // Done starts collapsed — Things 3 / Todoist.
                    initially_open: group != Group::Done,
                    match group {
                        Group::Today => {
                            let overdue = overdue_of(&items, today);
                            let rest: Vec<TaskInfo> = items
                                .iter()
                                .filter(|t| !overdue.iter().any(|o| o.id == t.id))
                                .cloned()
                                .collect();
                            rsx! {
                                if !overdue.is_empty() {
                                    OverdueBand { overdue: overdue.clone(), today, on_event: props.on_event }
                                    {rows(overdue)}
                                }
                                if !rest.is_empty() {
                                    {rows(rest)}
                                }
                            }
                        }
                        Group::Anytime => {
                            let homes = by_home(items, &props.anchors);
                            let headed = homes.len() > 1;
                            rsx! {
                                for (home, items) in homes.into_iter() {
                                    div { key: "{home.clone().unwrap_or_default()}", class: "flex flex-col gap-0.5",
                                        if headed {
                                            div { class: "px-1 pt-1.5 text-xs font-medium text-muted-foreground",
                                                {home.clone().unwrap_or_else(|| "No project".to_owned())}
                                            }
                                        }
                                        {rows(items)}
                                    }
                                }
                            }
                        }
                        _ => rows(items),
                    }
                }
            }
        }
    }
}

/// The head of Today when work is late: how much, and the one action
/// that clears the backlog without opening every task.
#[component]
fn OverdueBand(
    overdue: Vec<TaskInfo>,
    today: NaiveDate,
    on_event: EventHandler<TaskMutation>,
) -> Element {
    let n = overdue.len();
    let tomorrow = today.succ_opt().unwrap_or(today);
    let to_today = overdue.clone();
    let to_tomorrow = overdue;
    let btn = "rounded-md px-2.5 py-1.5 text-xs font-medium text-foreground ring-1 ring-inset ring-rose-500/40 transition-colors hover:bg-rose-500/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
    rsx! {
        div { class: "flex flex-wrap items-center justify-between gap-2 rounded-lg bg-rose-500/10 px-3 py-2",
            span { class: "text-sm font-semibold text-foreground",
                span { class: "text-rose-500", "{n} overdue" }
                span { class: "font-normal text-muted-foreground", " · oldest first" }
            }
            div { class: "flex items-center gap-1.5",
                button {
                    r#type: "button",
                    class: "{btn}",
                    onclick: move |_| reschedule(&to_today, today, on_event),
                    "Move all to today"
                }
                button {
                    r#type: "button",
                    class: "{btn}",
                    onclick: move |_| reschedule(&to_tomorrow, tomorrow, on_event),
                    "Tomorrow"
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct RowsProps {
    items: Vec<TaskInfo>,
    #[props(default)]
    anchors: Vec<(Uuid, AnchorChip)>,
    #[props(default)]
    org_of: Vec<(Uuid, String)>,
    handlers: Handlers,
}

#[component]
fn Rows(props: RowsProps) -> Element {
    // Subtasks nest under their parent when both are in the bucket —
    // the same domain arrangement the CLI list prints.
    let arranged = task_proto::arrange_families(props.items.clone(), |t| t.id, TaskDisplay::parent);
    let h = props.handlers;
    rsx! {
        div { class: "flex flex-col gap-0.5 pl-1",
            for (depth, t) in arranged.into_iter() {
                div {
                    key: "{t.id}",
                    // Subtasks hang off a hairline rail — family
                    // membership as structure, not fat indentation.
                    class: if depth > 0 { "ml-2.5 border-l border-border/60 pl-2.5" } else { "" },
                    TaskRow {
                        anchor: props.anchors.iter().find(|(id, _)| *id == t.id).map(|(_, c)| c.clone()),
                        org: props.org_of.iter().find(|(id, _)| *id == t.id).map(|(_, o)| o.clone()),
                        task: t,
                        on_toggle: h.on_toggle,
                        on_open: h.on_open,
                        on_track: h.on_track,
                        on_event: h.on_event,
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct SectionProps {
    id: &'static str,
    label: &'static str,
    count: usize,
    initially_open: bool,
    children: Element,
}

#[component]
fn Section(props: SectionProps) -> Element {
    let mut open = use_signal(|| props.initially_open);
    let icon_rotation = if open() { "rotate-90" } else { "" };
    rsx! {
        div { id: "{props.id}", class: "flex scroll-mt-4 flex-col gap-1",
            button {
                r#type: "button",
                class: "flex items-center gap-1 self-start rounded-sm py-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                aria_expanded: "{open()}",
                onclick: move |_| open.toggle(),
                span { class: "transition-transform {icon_rotation}", aria_hidden: "true",
                    ChevronRight { size: 12 }
                }
                span { "{props.label}" }
                span { class: "ml-1 rounded-full bg-muted px-1.5 py-0.5 text-xs leading-none text-muted-foreground",
                    "{props.count}"
                }
            }
            if open() {
                {props.children}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(title: &str, status: &str, due: Option<&str>, project: Option<&str>) -> TaskInfo {
        let mut t = TaskInfo::new(title);
        t.status = status.into();
        t.due = due.map(str::to_owned);
        if let Some(p) = project {
            t.projects.push(format!("[[{p}]]"));
        }
        t
    }

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    /// Overdue work is Today's, never folded or filed under "no date";
    /// in-progress work is Today's whatever its date.
    #[test]
    fn today_holds_overdue_due_today_and_in_progress() {
        let today = day("2026-10-06");
        assert_eq!(
            classify(&task("a", "open", Some("2026-08-27"), None), today),
            Group::Today
        );
        assert_eq!(
            classify(&task("b", "open", Some("2026-10-06"), None), today),
            Group::Today
        );
        assert_eq!(
            classify(&task("c", "in-progress", None, None), today),
            Group::Today
        );
        assert_eq!(
            classify(&task("d", "in-progress", Some("2026-12-01"), None), today),
            Group::Today
        );
        assert_eq!(
            classify(&task("e", "open", Some("2026-10-07"), None), today),
            Group::Upcoming
        );
        assert_eq!(
            classify(&task("f", "open", None, None), today),
            Group::Anytime
        );
        assert_eq!(
            classify(&task("g", "done", Some("2026-08-27"), None), today),
            Group::Done
        );
    }

    #[test]
    fn the_oldest_date_leads_and_undated_work_follows() {
        let mut bucket = vec![
            task("due today", "open", Some("2026-10-06"), None),
            task("working", "in-progress", None, None),
            task("overdue", "open", Some("2026-08-27"), None),
            task("undated", "open", None, None),
        ];
        sort_bucket(&mut bucket);
        let titles: Vec<&str> = bucket.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["overdue", "due today", "working", "undated"]);
    }

    #[test]
    fn the_overdue_band_holds_only_past_due_work() {
        let today = day("2026-10-06");
        let items = vec![
            task("overdue", "open", Some("2026-08-27"), None),
            task("due today", "open", Some("2026-10-06"), None),
            task("working", "in-progress", None, None),
        ];
        let band = overdue_of(&items, today);
        assert_eq!(band.len(), 1);
        assert_eq!(band[0].title, "overdue");
    }

    /// Anytime groups by project alphabetically, with the project-less
    /// group last.
    #[test]
    fn anytime_groups_by_project_with_the_unfiled_last() {
        let homes = by_home(
            vec![
                task("loose", "open", None, None),
                task("b1", "open", None, Some("Track Two")),
                task("a1", "open", None, Some("First Single")),
                task("b2", "open", None, Some("Track Two")),
            ],
            &[],
        );
        let names: Vec<Option<&str>> = homes.iter().map(|(h, _)| h.as_deref()).collect();
        assert_eq!(names, [Some("First Single"), Some("Track Two"), None]);
        assert_eq!(homes[1].1.len(), 2);
    }
}
