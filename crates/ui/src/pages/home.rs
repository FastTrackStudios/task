//! `/home` — the dashboard.
//!
//! A morning-briefing layout instead of a second task list:
//!
//! - **Header** — time-of-day greeting + the day's numbers (open
//!   tasks, due today, active projects).
//! - **Quick actions** — the four doors: new note (the `<space> n`
//!   flow, straight into naming), search everything, capture, tasks.
//! - **Today** — the open tasks due today or overdue, behind the same
//!   three-state checkbox as the board.
//! - **Projects** — each active project's pulse (status, done/total
//!   progress) and its first action. Capped; the full grid lives at
//!   `/projects`.
//!
//! Store-backed like every route page (`plans/atom-store-migration.md`)
//! — checkbox clicks are optimistic `TaskMutations` against the shared
//! task store, so the board and the dashboard can't disagree.

use crate::format::status_variant;
use architect_ui::lucide_dioxus::{
    ArrowRight, CalendarDays, CircleCheck, NotebookPen, Search, Zap,
};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use project_proto::ProjectInfo;
use task_proto::TaskInfo as DbTask;

use crate::routes::Route;
use crate::stores;
use crate::task_sort::{belongs, is_active, is_open_task, priority_rank};

/// One dashboard card's data: an active project with its task tally
/// and single next action. Derived in a memo so the
/// filter/sort/next-task walk over every project × task re-runs on
/// data changes only, not every render.
#[derive(Clone, PartialEq)]
struct Card {
    project: ProjectInfo,
    next: DbTask,
    done: usize,
    total: usize,
}

/// Projects shown on the dashboard before the "All projects" door.
const MAX_CARDS: usize = 9;
/// Rows in the Today rail.
const MAX_TODAY: usize = 8;

#[component]
pub fn HomeView() -> Element {
    let projects = stores::use_project_list();
    let tasks = stores::use_task_list();
    let muts = stores::use_task_mutations();
    let project_muts = stores::use_project_mutations();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let project_store = stores::use_project_store();
    let task_store = stores::use_task_store();

    let nav = use_navigator();
    let pending_title = use_context::<crate::chrome::PendingTitleEdit>().0;
    let mut search_open = use_context::<crate::chrome::SearchOpen>().0;
    let mut fleeting = use_context::<crate::chrome::FleetingOpen>().0;

    let cards = use_memo(move || {
        let projects = project_store.list();
        let tasks = task_store.list();
        let project_refs: Vec<&ProjectInfo> = projects.iter().map(|r| &r.project).collect();
        let task_refs: Vec<&DbTask> = tasks.iter().map(|r| &r.task).collect();
        build_cards(&project_refs, &task_refs)
    });

    // The Today rail: open tasks due today or overdue, soonest first.
    let today_tasks = use_memo(move || {
        let today = chrono::Local::now().date_naive();
        let mut due: Vec<DbTask> = task_store
            .list()
            .iter()
            .map(|r| r.task.clone())
            .filter(|t| is_open_task(t))
            .filter(|t| {
                t.due
                    .as_deref()
                    .and_then(|d| chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
                    .is_some_and(|d| d <= today)
            })
            .collect();
        due.sort_by(|a, b| {
            a.due
                .cmp(&b.due)
                .then_with(|| priority_rank(&a.priority).cmp(&priority_rank(&b.priority)))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        due
    });

    // The day's numbers for the header line.
    let stats = use_memo(move || {
        let rows = task_store.list();
        let open = rows.iter().filter(|r| is_open_task(&r.task)).count();
        let due_today = today_tasks.read().len();
        let active_projects = project_store
            .list()
            .iter()
            .filter(|r| !r.project.archived && is_active(&r.project.status))
            .count();
        (open, due_today, active_projects)
    });

    let on_status = move |(id, status): (uuid::Uuid, String)| {
        muts.apply(
            &crate::orgs::create_target(&selection.read(), &org_list.read()),
            task_ui::TaskMutation::SetStatus { id, status },
        );
    };

    let new_note = move |_| {
        let slug = crate::orgs::active_slug(&selection.read(), &org_list.read());
        if !slug.is_empty() {
            crate::shortcuts::spawn_new_note(slug, nav, pending_title);
        }
    };

    let view = match (
        projects.value().as_ref(),
        tasks.value().as_ref(),
        projects.error().or(tasks.error()),
    ) {
        (Some(_), Some(_), _) => {
            let (open, due_today, active_projects) = stats();
            let org_choices: Vec<(String, String)> = org_list
                .read()
                .iter()
                .map(|o| (o.slug.clone(), o.name.clone()))
                .collect();
            let quick_add = rsx! {
                super::projects::ProjectQuickAdd {
                    compact: true,
                    orgs: org_choices,
                    default_slug: crate::orgs::create_target(&selection.read(), &org_list.read()),
                    on_create: move |(slug, title): (String, String)| {
                        project_muts.create(slug, stores::draft_project(title));
                    },
                }
            };
            let card_list = cards();
            let shown: Vec<Card> = card_list.iter().take(MAX_CARDS).cloned().collect();
            let overflow = card_list.len().saturating_sub(shown.len());
            let today_list: Vec<DbTask> =
                today_tasks.read().iter().take(MAX_TODAY).cloned().collect();
            let today_overflow = today_tasks.read().len().saturating_sub(today_list.len());

            rsx! {
                // ── Header: greeting + the day's numbers ──────────
                div { class: "flex flex-wrap items-end justify-between gap-x-4 gap-y-2",
                    div { class: "flex flex-col gap-0.5",
                        Heading { level: HeadingLevel::H1, class: "tracking-tight", "{greeting()}" }
                        span { class: "text-sm text-muted-foreground",
                            {chrono::Local::now().format("%A, %B %-d").to_string()}
                        }
                    }
                    div { class: "flex items-center gap-2",
                        StatChip { value: open, label: "open" }
                        StatChip { value: due_today, label: "due today" }
                        StatChip { value: active_projects, label: "projects" }
                    }
                }

                // ── Quick actions ─────────────────────────────────
                div { class: "grid grid-cols-2 gap-2 sm:grid-cols-4",
                    ActionTile {
                        label: "New note",
                        hint: "Space n",
                        onclick: new_note,
                        NotebookPen { size: 16 }
                    }
                    ActionTile {
                        label: "Search",
                        hint: "Space Space",
                        onclick: move |_| search_open.set(true),
                        Search { size: 16 }
                    }
                    ActionTile {
                        label: "Capture",
                        hint: "Space c",
                        onclick: move |_| fleeting.set(true),
                        Zap { size: 16 }
                    }
                    ActionTile {
                        label: "Tasks",
                        hint: "g t",
                        onclick: move |_| { nav.push(Route::TasksRoute {}); },
                        CircleCheck { size: 16 }
                    }
                }

                // ── Today + Projects ──────────────────────────────
                div { class: "grid grid-cols-1 items-start gap-4 lg:grid-cols-3",
                    // Today rail — first on mobile, right rail on desktop.
                    div { class: "order-first flex flex-col gap-1.5 rounded-xl border border-border/70 bg-card/50 p-3.5 lg:order-last",
                        div { class: "flex items-center justify-between",
                            span { class: "text-xs font-semibold uppercase tracking-wider text-muted-foreground",
                                "Today"
                            }
                            if due_today > 0 {
                                span { class: "rounded-full bg-muted/50 px-1.5 text-[10px] tabular-nums text-muted-foreground",
                                    "{due_today}"
                                }
                            }
                        }
                        if today_list.is_empty() {
                            div { class: "flex items-center gap-2 py-3 text-sm text-muted-foreground",
                                CircleCheck { size: 15 }
                                "Nothing due — clear runway."
                            }
                        }
                        for t in today_list.into_iter() {
                            TodayRow { key: "{t.id}", task: t, on_status }
                        }
                        if today_overflow > 0 {
                            Link {
                                to: Route::TasksRoute {},
                                class: "mt-1 text-xs text-muted-foreground hover:text-foreground",
                                "{today_overflow} more on the board →"
                            }
                        }
                    }
                    // Projects — the pulse grid.
                    div { class: "flex flex-col gap-2.5 lg:col-span-2",
                        div { class: "flex items-center justify-between gap-3",
                            span { class: "text-xs font-semibold uppercase tracking-wider text-muted-foreground",
                                "Active work"
                            }
                            div { class: "flex items-center gap-2",
                                {quick_add}
                                Link {
                                    to: Route::ProjectsRoute {},
                                    class: "flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground",
                                    if overflow > 0 { "All projects · {card_list.len()}" } else { "All projects" }
                                    ArrowRight { size: 12 }
                                }
                            }
                        }
                        if shown.is_empty() {
                            div { class: "flex flex-col items-center gap-2 rounded-xl border border-dashed border-border/70 bg-card/40 px-6 py-10 text-center",
                                CircleCheck { size: 20 }
                                Text { variant: TextVariant::Muted, "No active projects have open tasks right now." }
                            }
                        }
                        div { class: "grid grid-cols-1 gap-2.5 md:grid-cols-2",
                            for card in shown.into_iter() {
                                ProjectCard {
                                    key: "{card.project.id}",
                                    project: card.project,
                                    next: card.next,
                                    done: card.done,
                                    total: card.total,
                                    on_status: move |(id, status)| on_status((id, status)),
                                }
                            }
                        }
                    }
                }
            }
        }
        (_, _, Some(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't load your workspace",
                message: e,
                on_retry: move |()| {
                    project_store.reload();
                    task_store.reload();
                },
            }
        },
        _ => render_loading(),
    };

    rsx! {
        div { class: "mx-auto flex w-full max-w-6xl flex-col gap-5 p-3 sm:p-5 lg:px-8 lg:py-6",
            {view}
        }
    }
}

/// Time-of-day greeting for the header.
fn greeting() -> &'static str {
    match chrono::Local::now().format("%H").to_string().parse::<u8>() {
        Ok(h) if h < 5 => "Up late",
        Ok(h) if h < 12 => "Good morning",
        Ok(h) if h < 17 => "Good afternoon",
        _ => "Good evening",
    }
}

/// One number + label in the header's stat strip.
#[component]
fn StatChip(value: usize, label: &'static str) -> Element {
    rsx! {
        div { class: "flex items-baseline gap-1.5 rounded-lg border border-border/60 bg-card/50 px-2.5 py-1.5",
            span { class: "text-sm font-semibold tabular-nums text-foreground", "{value}" }
            span { class: "text-[11px] text-muted-foreground", "{label}" }
        }
    }
}

/// One quick-action door: icon, label, and its key sequence.
#[component]
fn ActionTile(
    label: &'static str,
    hint: &'static str,
    onclick: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "group flex items-center gap-2.5 rounded-xl border border-border/70 bg-card/60 px-3 py-2.5 text-left transition-colors hover:border-border hover:bg-accent/40",
            onclick: move |e| onclick.call(e),
            span { class: "flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted/50 text-muted-foreground transition-colors group-hover:text-foreground",
                {children}
            }
            span { class: "min-w-0 flex-1 truncate text-sm font-medium text-foreground", "{label}" }
            kbd { class: "hidden shrink-0 rounded border border-border/60 bg-muted/40 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground sm:inline",
                "{hint}"
            }
        }
    }
}

/// One Today-rail row: live checkbox, title, due pill.
#[component]
fn TodayRow(task: DbTask, on_status: EventHandler<(uuid::Uuid, String)>) -> Element {
    let due = task.due.as_deref().and_then(parse_due);
    let id = task.id;
    let status = task.status.clone();
    let ui_status = task_ui::Status::from_str(&task.status).unwrap_or(task_ui::Status::Open);
    let ui_priority =
        task_ui::Priority::from_str(&task.priority).unwrap_or(task_ui::Priority::Normal);
    rsx! {
        div { class: "flex items-center gap-2.5 rounded-lg px-1 py-1 transition-colors hover:bg-accent/30",
            task_ui::CheckboxButton {
                status: ui_status,
                priority: ui_priority,
                on_click: move |()| {
                    let s = task_proto::click_transition(&status, None);
                    on_status.call((id, s.to_string()));
                },
            }
            Link {
                to: Route::TaskDetailRoute { id },
                class: "min-w-0 flex-1 truncate text-sm text-foreground hover:underline",
                "{task.title}"
            }
            if let Some((label, cls)) = due {
                span { class: "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[11px] {cls}",
                    CalendarDays { size: 11 }
                    "{label}"
                }
            }
        }
    }
}

fn render_loading() -> Element {
    rsx! {
        div { class: "flex flex-col gap-4",
            div { class: "h-8 w-56 animate-pulse rounded-md bg-muted" }
            div { class: "grid grid-cols-2 gap-2 sm:grid-cols-4",
                for i in 0..4 {
                    div { key: "{i}", class: "h-12 animate-pulse rounded-xl bg-muted" }
                }
            }
            div { class: "grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3",
                for i in 0..6 {
                    div { key: "{i}", class: "flex flex-col gap-3 rounded-xl border border-border/70 bg-card p-4",
                        div { class: "h-5 w-40 animate-pulse rounded-md bg-muted" }
                        div { class: "h-1.5 w-full animate-pulse rounded-full bg-muted" }
                        div { class: "h-4 w-full animate-pulse rounded-md bg-muted" }
                    }
                }
            }
        }
    }
}

/// Each active project with its task tally + single next action;
/// projects with nothing open drop out — the dashboard is only
/// "what's next".
fn build_cards(projects: &[&ProjectInfo], tasks: &[&DbTask]) -> Vec<Card> {
    let mut cards: Vec<Card> = projects
        .iter()
        .filter(|p| !p.archived && is_active(&p.status))
        .filter_map(|p| {
            let mine: Vec<&&DbTask> = tasks.iter().filter(|t| belongs(t, p)).collect();
            let total = mine.len();
            let done = mine.iter().filter(|t| !is_open_task(t)).count();
            next_task(p, tasks).map(|t| Card {
                project: (*p).clone(),
                next: t.clone(),
                done,
                total,
            })
        })
        .collect();
    // Soonest due first (undated last), then project title.
    cards.sort_by(|a, b| {
        match (a.next.due.as_deref(), b.next.due.as_deref()) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| {
            a.project
                .title
                .to_lowercase()
                .cmp(&b.project.title.to_lowercase())
        })
    });
    cards
}

/// One project's pulse: title + status, a thin done/total progress
/// bar, and the first action behind a live checkbox.
#[component]
fn ProjectCard(
    project: ProjectInfo,
    next: DbTask,
    done: usize,
    total: usize,
    on_status: EventHandler<(uuid::Uuid, String)>,
) -> Element {
    let pid = project.id.to_string();
    let pct = if total == 0 { 0 } else { done * 100 / total };
    let due = next.due.as_deref().and_then(parse_due);
    let next_id = next.id;
    let next_status = next.status.clone();
    let ui_status = task_ui::Status::from_str(&next.status).unwrap_or(task_ui::Status::Open);
    let ui_priority =
        task_ui::Priority::from_str(&next.priority).unwrap_or(task_ui::Priority::Normal);

    rsx! {
        div { class: "group flex flex-col gap-2.5 rounded-xl border border-border/70 bg-card/70 p-3.5 transition-colors hover:border-border",
            div { class: "flex items-center justify-between gap-2",
                Link {
                    to: Route::ProjectDetailRoute { id: pid },
                    class: "min-w-0 text-sm font-semibold text-foreground hover:underline",
                    span { class: "truncate", "{project.title}" }
                }
                StatusBadge {
                    variant: status_variant(&project.status),
                    label: project.status.clone(),
                }
            }
            // Done/total as a hairline bar — the project's pulse in
            // 6 vertical pixels.
            div { class: "flex items-center gap-2",
                div { class: "h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-muted/20",
                    div {
                        class: "h-full rounded-full bg-primary transition-[width]",
                        style: "width: {pct}%",
                    }
                }
                span { class: "shrink-0 text-[11px] tabular-nums text-muted-foreground",
                    "{done}/{total}"
                }
            }
            // The first action — live checkbox, same click cycle as
            // the board (start the clock, complete, reopen).
            div { class: "flex items-center gap-2.5 rounded-lg bg-muted/30 px-2.5 py-2",
                task_ui::CheckboxButton {
                    status: ui_status,
                    priority: ui_priority,
                    on_click: move |()| {
                        let s = task_proto::click_transition(&next_status, None);
                        on_status.call((next_id, s.to_string()));
                    },
                }
                span { class: "min-w-0 flex-1 truncate text-sm text-foreground", "{next.title}" }
                if let Some((label, cls)) = due {
                    span { class: "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[11px] {cls}",
                        CalendarDays { size: 11 }
                        "{label}"
                    }
                }
            }
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────────

/// The single next task for a project: open, soonest due (None last),
/// then highest priority, then title.
fn next_task<'a>(p: &ProjectInfo, tasks: &[&'a DbTask]) -> Option<&'a DbTask> {
    let mut candidates: Vec<&DbTask> = tasks
        .iter()
        .filter(|t| is_open_task(t) && belongs(t, p))
        .copied()
        .collect();
    candidates.sort_by(|a, b| {
        match (a.due.as_deref(), b.due.as_deref()) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| priority_rank(&a.priority).cmp(&priority_rank(&b.priority)))
        .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    candidates.into_iter().next()
}

/// Format an ISO `YYYY-MM-DD` due string into a relative label + a
/// token-based pill class (overdue → destructive, today → amber).
fn parse_due(s: &str) -> Option<(String, &'static str)> {
    let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    let today = chrono::Local::now().date_naive();
    let label = if d == today {
        "Today".to_string()
    } else if d == today + chrono::Duration::days(1) {
        "Tomorrow".to_string()
    } else {
        d.format("%b %-d").to_string()
    };
    let cls = match d.cmp(&today) {
        std::cmp::Ordering::Less => {
            "border border-destructive/50 bg-destructive/15 text-destructive"
        }
        std::cmp::Ordering::Equal => "border border-amber-400/50 bg-amber-500/15 text-amber-200",
        std::cmp::Ordering::Greater => "border border-border bg-muted/40 text-muted-foreground",
    };
    Some((label, cls))
}
