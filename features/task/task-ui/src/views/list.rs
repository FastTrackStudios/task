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
    pub on_toggle: EventHandler<Uuid>,
    pub on_open: EventHandler<Uuid>,
    pub on_event: EventHandler<TaskMutation>,
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

/// In progress first, then by date (overdue before today, soonest
/// upcoming first), then priority, then title.
fn sort_bucket(bucket: &mut [TaskInfo]) {
    bucket.sort_by(|a, b| {
        let working = |t: &TaskInfo| t.status_enum() != Status::InProgress;
        working(a)
            .cmp(&working(b))
            .then_with(|| a.due_date().cmp(&b.due_date()))
            .then_with(|| priority_rank(a.priority_enum()).cmp(&priority_rank(b.priority_enum())))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
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

    rsx! {
        div { class: "flex min-w-0 flex-col gap-4",
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
                    if group == Group::Anytime {
                        {
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
                                        Rows { items, anchors: props.anchors.clone(), on_toggle: props.on_toggle, on_open: props.on_open, on_event: props.on_event }
                                    }
                                }
                            }
                        }
                    } else {
                        Rows { items, anchors: props.anchors.clone(), on_toggle: props.on_toggle, on_open: props.on_open, on_event: props.on_event }
                    }
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
    on_toggle: EventHandler<Uuid>,
    on_open: EventHandler<Uuid>,
    on_event: EventHandler<TaskMutation>,
}

#[component]
fn Rows(props: RowsProps) -> Element {
    // Subtasks nest under their parent when both are in the bucket —
    // the same domain arrangement the CLI list prints.
    let arranged = task_proto::arrange_families(props.items.clone(), |t| t.id, TaskDisplay::parent);
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
                        task: t,
                        on_toggle: props.on_toggle,
                        on_open: props.on_open,
                        on_event: props.on_event,
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
                class: "flex items-center gap-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground hover:text-foreground py-1",
                onclick: move |_| open.toggle(),
                span { class: "transition-transform {icon_rotation}",
                    ChevronRight { size: 12 }
                }
                span { "{props.label}" }
                span { class: "ml-1 rounded-full bg-muted/50 px-1.5 py-0 text-[10px] text-muted-foreground",
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
    fn in_progress_leads_then_the_oldest_date() {
        let mut bucket = vec![
            task("due today", "open", Some("2026-10-06"), None),
            task("overdue", "open", Some("2026-08-27"), None),
            task("working", "in-progress", None, None),
        ];
        sort_bucket(&mut bucket);
        let titles: Vec<&str> = bucket.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["working", "overdue", "due today"]);
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
