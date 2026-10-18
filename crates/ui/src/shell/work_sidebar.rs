//! The work sidebar — what the explorer column shows wherever the
//! person is planning or doing work (Home, Tasks, Projects, Schedule,
//! Inbox, Timer).
//!
//! The vault tree is the right companion inside notes and wikis, and
//! the wrong one here: beside a task list it is a column of folders
//! that answers nothing being asked. This column answers "what is on
//! my plate": the inbox and tasks with live counts (their filters live
//! on those pages, not here), every active project with how much is open
//! in it, and every other section of the workspace.
//!
//! Its counts come from the same rules as the list itself — the
//! person's filters ([`crate::task_board::filter`]) and the same section
//! rule ([`task_ui::task_section`]) — so a sidebar number and the list
//! beneath it can't disagree.

use dioxus::prelude::*;
use task_ui::TaskSection;

use crate::routes::Route;
use crate::stores;
use crate::task_sort::is_active;

#[derive(Clone, PartialEq)]
struct Counts {
    /// Open work due today or before — the Tasks row's number.
    today: usize,
    overdue: usize,
    /// `(project id, title, open tasks)`, active projects by title.
    projects: Vec<(String, String, usize)>,
}

#[component]
pub fn WorkSidebar(current: Route) -> Element {
    // Subscribing loads the lists if no page has yet.
    let _tasks = stores::use_task_list();
    let _projects = stores::use_project_list();
    let _sessions = stores::use_session_list();
    let task_store = stores::use_task_store();
    let project_store = stores::use_project_store();
    let session_store = stores::use_session_store();
    let prefs = use_context::<crate::prefs::PrefsCtx>().prefs;
    // The same hour as the Tasks page, so its counts and these agree.
    let focus = crate::now_block::use_focus();
    let account = use_context::<crate::auth::AuthCtx>().active;

    let counts = use_memo(move || {
        let me = account.read().as_ref().map(|a| a.user_id.to_string());
        let today = chrono::Local::now().date_naive();
        let p = prefs.read();
        let (active_only, relevant_only, location) =
            (p.tasks_active, p.tasks_relevant, p.location.clone());
        drop(p);
        let active_project = session_store
            .list()
            .iter()
            .find(|r| r.session.end_time.is_none())
            .and_then(|r| r.session.project_id);
        let focus = focus();
        let ctx = crate::task_board::relevance_ctx(&location, active_project, focus);
        let entries = task_store.entries();
        let all: Vec<&task_proto::TaskInfo> = stores::unique_tasks(&entries)
            .into_iter()
            .map(|r| &r.task)
            .collect();
        // Open work on the board, through the person's own filters.
        let board: Vec<&task_proto::TaskInfo> = crate::task_board::filter(
            &all,
            crate::task_board::Scope::Actions,
            me.as_deref(),
            active_only,
            relevant_only,
            &ctx,
        )
        .visible
        .into_iter()
        .filter(|t| task_proto::status_is_open(&t.status))
        .collect();
        let index = crate::task_board::ProjectIndex::new(
            project_store
                .list()
                .iter()
                .map(|r| (r.project.id, r.project.title.clone())),
        );
        let mut c = Counts {
            today: 0,
            overdue: 0,
            projects: Vec::new(),
        };
        for t in &board {
            if task_ui::task_section(t, today) == TaskSection::Today {
                c.today += 1;
                if task_ui::display::TaskDisplay::due_date(*t).is_some_and(|d| d < today) {
                    c.overdue += 1;
                }
            }
        }
        let mut rows: Vec<(String, String, usize)> = project_store
            .list()
            .iter()
            .filter(|r| !r.project.archived && is_active(&r.project.status))
            .map(|r| {
                let pid = r.project.id;
                let n = board
                    .iter()
                    .filter(|t| index.anchor(t) == Some(task_proto::Anchor::Project(pid)))
                    .count();
                (pid.to_string(), r.project.title.clone(), n)
            })
            .collect();
        rows.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        c.projects = rows;
        c
    });
    let c = counts();

    let on_tasks = matches!(current, Route::TasksRoute {});

    use architect_ui::lucide_dioxus as i;
    let to_process = crate::process::use_to_process_count();
    let on_inbox = matches!(current, Route::InboxRoute {});
    let favorites = use_context::<crate::favorites::Favorites>().0;
    let mut projects_open = use_signal(|| true);
    let mut favorites_open = use_signal(|| true);
    let on_projects = matches!(current, Route::ProjectsRoute {});
    let mut workspace_open = use_signal(|| true);
    let mut customize_open = use_signal(|| false);
    // Every section the rows above don't already hold, in the person's
    // arrangement (Customize) — the phone's bar keeps only four.
    let every_section = crate::nav::nav_tabs_for(&crate::nav::use_active_plugins());
    let layout = crate::nav_layout::use_nav_layout();
    let sections: Vec<crate::nav::NavTab> = layout
        .read()
        .arrange(every_section.clone())
        .0
        .into_iter()
        .filter(|t| {
            !matches!(
                t.route,
                Route::InboxRoute {} | Route::TasksRoute {} | Route::ProjectsRoute {}
            )
        })
        .collect();
    // Starred projects lead, then the rest of the active ones.
    let project_rows: Vec<(String, String, usize, bool)> = {
        let favs = favorites.read();
        let starred = |pid: &str| favs.iter().any(|f| f.kind == "project" && f.id == pid);
        let mut rows: Vec<_> = c
            .projects
            .iter()
            .map(|(pid, t, n)| (pid.clone(), t.clone(), *n, starred(pid)))
            .collect();
        rows.sort_by_key(|r| !r.3);
        rows
    };

    // A row: icon, label, count, the current one in the tab bar's pill.
    let row = |here: bool| {
        if here {
            "flex min-h-10 w-full items-center gap-3 rounded-[10px] bg-foreground/[0.08] px-2.5 text-[15px] font-medium text-foreground md:min-h-8 md:text-sm"
        } else {
            "flex min-h-10 w-full items-center gap-3 rounded-[10px] px-2.5 text-[15px] text-foreground/85 hover:bg-foreground/[0.05] active:bg-foreground/[0.08] md:min-h-8 md:text-sm"
        }
    };
    let icon = "flex size-5 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-[18px] md:[&_svg]:size-4";
    let count = "ml-auto shrink-0 text-[13px] tabular-nums text-muted-foreground md:text-xs";
    let heading = "flex w-full items-center gap-1 px-2.5 pb-1 text-[12px] font-semibold uppercase tracking-[0.06em] text-muted-foreground/80";

    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col",
            // ── The workspace ──
            div { class: "px-3 pb-2 pt-3",
                crate::shell::org_switcher::OrgSwitcher { compact: false, theme: false }
            }
            nav { class: "flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-2 pb-4 pt-1", aria_label: "Work",
                // ── Your work ──
                // Two rows, not a row per filter: Today, Upcoming,
                // Anytime and unfiled work are the Tasks page's own
                // sections and filters, one tap further in.
                div { class: "flex flex-col gap-0.5",
                    Link {
                        to: Route::InboxRoute {},
                        class: row(on_inbox),
                        span { class: icon, i::Inbox {} }
                        span { "Inbox" }
                        if to_process > 0 {
                            span { class: "ml-auto flex h-5 min-w-5 items-center justify-center rounded-full bg-primary px-1.5 text-[11px] font-semibold tabular-nums text-primary-foreground",
                                "{to_process}"
                            }
                        }
                    }
                    Link {
                        to: Route::TasksRoute {},
                        class: row(on_tasks),
                        span { class: icon, i::CircleCheck {} }
                        span { "Tasks" }
                        span { class: count,
                            if c.overdue > 0 {
                                span { class: "mr-1.5 rounded-full bg-rose-500/20 px-1.5 py-0.5 text-[11px] font-medium text-rose-300",
                                    "{c.overdue} late"
                                }
                            }
                            if c.today > 0 { "{c.today}" }
                        }
                    }
                }

                // ── Favorites ──
                if !favorites.read().is_empty() {
                    div { class: "flex flex-col gap-0.5",
                        button {
                            r#type: "button",
                            class: heading,
                            aria_expanded: "{favorites_open}",
                            onclick: move |_| favorites_open.toggle(),
                            "Favorites"
                            span { class: if favorites_open() { "transition-transform rotate-90" } else { "transition-transform" },
                                i::ChevronRight { size: 12 }
                            }
                        }
                        if favorites_open() {
                            for fav in favorites.read().iter().cloned() {
                                if let Some(route) = fav.route() {
                                    Link {
                                        key: "{fav.kind}:{fav.id}",
                                        to: route,
                                        class: row(false),
                                        span { class: icon,
                                            match fav.kind.as_str() {
                                                "project" => rsx! { i::Box {} },
                                                "page" => rsx! { i::FileText {} },
                                                _ => rsx! { i::CircleCheck {} },
                                            }
                                        }
                                        span { class: "min-w-0 truncate", "{fav.title}" }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Projects ──
                // The row opens the Projects page; the chevron folds
                // the starred-then-active list under it.
                div { class: "flex flex-col gap-0.5",
                    div { class: "flex items-center gap-0.5",
                        Link {
                            to: Route::ProjectsRoute {},
                            class: "{row(on_projects)} min-w-0 flex-1",
                            span { class: icon, i::FolderKanban {} }
                            span { "Projects" }
                            span { class: count, "{c.projects.len()}" }
                        }
                        button {
                            r#type: "button",
                            "data-testid": "sidebar-projects-toggle",
                            class: "flex size-10 shrink-0 items-center justify-center rounded-[10px] text-muted-foreground hover:bg-foreground/[0.05] active:bg-foreground/[0.08] md:size-8",
                            aria_expanded: "{projects_open}",
                            aria_label: if projects_open() { "Hide projects" } else { "Show projects" },
                            onclick: move |_| projects_open.toggle(),
                            span { class: if projects_open() { "transition-transform rotate-90" } else { "transition-transform" },
                                i::ChevronRight { size: 15 }
                            }
                        }
                    }
                    if projects_open() {
                        if c.projects.is_empty() {
                            p { class: "px-2.5 text-[13px] text-muted-foreground", "No active projects." }
                        }
                        for (pid, title, n, starred) in project_rows.iter().cloned() {
                            {
                                let here = matches!(&current, Route::ProjectDetailRoute { id } if *id == pid);
                                rsx! {
                                    Link {
                                        key: "{pid}",
                                        to: Route::ProjectDetailRoute { id: pid.clone() },
                                        class: if here {
                                            "flex min-h-12 w-full items-center gap-3 rounded-[12px] bg-foreground/[0.08] px-2 text-[15px] font-medium text-foreground md:min-h-10 md:text-sm"
                                        } else {
                                            "flex min-h-12 w-full items-center gap-3 rounded-[12px] px-2 text-[15px] text-foreground/85 hover:bg-foreground/[0.05] active:bg-foreground/[0.08] md:min-h-10 md:text-sm"
                                        },
                                        // A cover, not an icon: big enough to
                                        // tell the projects apart at a glance.
                                        img {
                                            class: "size-9 shrink-0 rounded-[8px] object-cover md:size-7 md:rounded-[6px]",
                                            src: "{task_ui_core::abstract_art::data_url(&pid)}",
                                            alt: "",
                                        }
                                        span { class: "min-w-0 truncate", "{title}" }
                                        if starred {
                                            span { class: "shrink-0 text-amber-400", aria_label: "Starred", i::Star { size: 12 } }
                                        }
                                        if n > 0 { span { class: count, "{n}" } }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Every other section ──
                // The phone's bar holds four; the rest live here (the
                // old switcher menu's job), in the person's order.
                div { class: "flex flex-col gap-0.5",
                    button {
                        r#type: "button",
                        class: heading,
                        aria_expanded: "{workspace_open}",
                        onclick: move |_| workspace_open.toggle(),
                        "Workspace"
                        span { class: if workspace_open() { "transition-transform rotate-90" } else { "transition-transform" },
                            i::ChevronRight { size: 12 }
                        }
                    }
                    if workspace_open() {
                        for tab in sections.iter().cloned() {
                            Link {
                                key: "{tab.label}",
                                to: tab.route.clone(),
                                class: row(crate::nav::tabs_match(&current, &tab)),
                                span { class: icon, {(tab.icon)()} }
                                span { class: "min-w-0 truncate", "{tab.label}" }
                            }
                        }
                        button {
                            r#type: "button",
                            "data-testid": "customize-tab-bar",
                            class: "{row(false)} md:hidden",
                            onclick: move |_| customize_open.set(true),
                            span { class: icon, i::SlidersHorizontal {} }
                            span { "Customize tab bar" }
                        }
                    }
                }
            }
            AccountFooter {}
        }
        crate::nav_layout::CustomizeSheet {
            open: customize_open(),
            on_close: move |()| customize_open.set(false),
            tabs: every_section,
            layout,
        }
    }
}

/// The sidebar's foot: who is signed in, their status, and where this
/// device's data lives. Opens the Account page on a phone and Settings
/// on a desk.
#[component]
fn AccountFooter() -> Element {
    let ctx = use_context::<crate::auth::AuthCtx>();
    let local = use_context::<crate::presence::PresenceLocal>();
    let account = ctx.active.read().clone();
    let (name, email) = account.as_ref().map_or_else(
        || ("Not signed in".to_owned(), String::new()),
        |a| (a.name.clone(), a.email.clone()),
    );
    let status = local.effective_status();
    let phone =
        try_use_context::<crate::shell::phone_gestures::PhonePanels>().is_some_and(|p| (p.phone)());
    let nav = use_navigator();
    rsx! {
        // No safe-area inset here: the phone drawer already keeps clear of
        // the home indicator, and both together doubled the gap.
        div { class: "border-t border-border/60 p-2 pb-1 md:pb-2",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-3 rounded-[10px] px-2 py-2 text-left hover:bg-foreground/[0.05] active:bg-foreground/[0.08]",
                onclick: move |_| {
                    if phone {
                        nav.push(Route::AccountRoute {});
                    } else {
                        nav.push(Route::SettingsRoute {});
                    }
                },
                span { class: "relative shrink-0",
                    task_ui_core::avatar::Avatar { name: name.clone(), email: email.clone(), size: 32 }
                    span { class: "absolute -bottom-0.5 -right-0.5 size-3 rounded-full ring-2 ring-popover {status.dot_class()}" }
                }
                span { class: "flex min-w-0 flex-1 flex-col",
                    span { class: "truncate text-[15px] font-medium text-foreground md:text-sm", "{name}" }
                    span { class: "truncate text-[12px] text-muted-foreground", "{status.label()}" }
                }
                span { class: "text-muted-foreground", architect_ui::lucide_dioxus::ChevronsUpDown { size: 16 } }
            }
        }
    }
}
