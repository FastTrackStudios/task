//! The work sidebar — what the explorer column shows wherever the
//! person is planning or doing work (Home, Tasks, Projects, Schedule,
//! Inbox, Timer, Goals, Milestones, Gantt).
//!
//! The vault tree is the right companion inside notes and wikis, and
//! the wrong one here: beside a task list it is a column of folders
//! that answers nothing being asked. This column answers "what is on
//! my plate": the task list's own sections with live counts (the same
//! rule the list sorts by, [`task_ui::task_section`]), and every active
//! project with how much is open in it.

use dioxus::prelude::*;
use task_ui::TaskSection;

use crate::routes::Route;
use crate::stores;
use crate::task_sort::{is_active, is_open_task};

/// Scroll the Tasks page to one of its sections, waiting for the list
/// to render when the click also navigated there.
fn scroll_to(anchor: &'static str) {
    let _ = dioxus::document::eval(&format!(
        "(function go(n){{const e=document.getElementById('{anchor}');\
         if(e){{e.scrollIntoView({{behavior:'smooth',block:'start'}});}}\
         else if(n>0){{setTimeout(()=>go(n-1),100);}}}})(30);"
    ));
}

#[derive(Clone, PartialEq)]
struct Counts {
    today: usize,
    overdue: usize,
    upcoming: usize,
    anytime: usize,
    unfiled: usize,
    /// `(project id, title, open tasks)`, active projects by title.
    projects: Vec<(String, String, usize)>,
}

#[component]
pub fn WorkSidebar(current: Route) -> Element {
    // Subscribing loads the lists if no page has yet.
    let _tasks = stores::use_task_list();
    let _projects = stores::use_project_list();
    let task_store = stores::use_task_store();
    let project_store = stores::use_project_store();
    let nav = use_navigator();

    let counts = use_memo(move || {
        let today = chrono::Local::now().date_naive();
        let tasks = task_store.list();
        let open: Vec<_> = tasks
            .iter()
            .map(|r| &r.task)
            .filter(|t| is_open_task(t))
            .collect();
        let mut c = Counts {
            today: 0,
            overdue: 0,
            upcoming: 0,
            anytime: 0,
            unfiled: 0,
            projects: Vec::new(),
        };
        for t in &open {
            // The Tasks page lifts work that belongs to nothing out of
            // the list into its triage strip; count it the same way.
            if task_proto::anchor(t).is_none() && t.projects.is_empty() && t.contexts.is_empty() {
                c.unfiled += 1;
                continue;
            }
            match task_ui::task_section(t, today) {
                TaskSection::Today => {
                    c.today += 1;
                    if task_ui::display::TaskDisplay::due_date(*t).is_some_and(|d| d < today) {
                        c.overdue += 1;
                    }
                }
                TaskSection::Upcoming => c.upcoming += 1,
                TaskSection::Anytime => c.anytime += 1,
                TaskSection::Done => {}
            }
        }
        let projects = project_store.list();
        let mut rows: Vec<(String, String, usize)> = projects
            .iter()
            .filter(|r| !r.project.archived && is_active(&r.project.status))
            .map(|r| {
                let p = &r.project;
                let title = p.title.to_lowercase();
                let n = open
                    .iter()
                    .filter(|t| {
                        t.project_id == Some(p.id)
                            || t.projects.0.iter().any(|s| {
                                s.trim()
                                    .trim_start_matches("[[")
                                    .trim_end_matches("]]")
                                    .trim()
                                    .to_lowercase()
                                    == title
                            })
                    })
                    .count();
                (p.id.to_string(), p.title.clone(), n)
            })
            .collect();
        rows.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        c.projects = rows;
        c
    });
    let c = counts();

    let on_tasks = matches!(current, Route::TasksRoute {});
    let smart = [
        (TaskSection::Today, c.today),
        (TaskSection::Upcoming, c.upcoming),
        (TaskSection::Anytime, c.anytime),
    ];

    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-3",
            div { class: "flex flex-col gap-0.5",
                for (section, n) in smart {
                    button {
                        key: "{section.label()}",
                        r#type: "button",
                        class: "flex items-center justify-between rounded-md px-2 py-1.5 text-sm text-foreground/90 transition-colors hover:bg-accent/40",
                        onclick: move |_| {
                            if !on_tasks {
                                nav.push(Route::TasksRoute {});
                            }
                            scroll_to(section.anchor());
                        },
                        span { "{section.label()}" }
                        span { class: "flex items-center gap-1.5 text-xs tabular-nums text-muted-foreground",
                            if section == TaskSection::Today && c.overdue > 0 {
                                span {
                                    class: "rounded-full bg-destructive/15 px-1.5 text-destructive",
                                    title: "overdue",
                                    "{c.overdue}"
                                }
                            }
                            "{n}"
                        }
                    }
                }
                if c.unfiled > 0 {
                    button {
                        r#type: "button",
                        class: "flex items-center justify-between rounded-md px-2 py-1.5 text-sm text-foreground/90 transition-colors hover:bg-accent/40",
                        onclick: move |_| {
                            if !on_tasks {
                                nav.push(Route::TasksRoute {});
                            }
                        },
                        span { "Unfiled" }
                        span { class: "text-xs tabular-nums text-muted-foreground", "{c.unfiled}" }
                    }
                }
            }

            div { class: "flex flex-col gap-1",
                div { class: "flex items-center justify-between px-2",
                    span { class: "text-[10px] font-semibold uppercase tracking-widest text-muted-foreground",
                        "Projects"
                    }
                    Link {
                        to: Route::ProjectsRoute {},
                        class: "text-[11px] text-muted-foreground hover:text-foreground",
                        "All ›"
                    }
                }
                if c.projects.is_empty() {
                    p { class: "px-2 text-xs text-muted-foreground", "No active projects." }
                }
                div { class: "flex flex-col",
                    for (pid, title, n) in c.projects.iter().cloned() {
                        Link {
                            key: "{pid}",
                            to: Route::ProjectDetailRoute { id: pid.clone() },
                            class: "flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-sm text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground",
                            span { class: "min-w-0 truncate", "{title}" }
                            if n > 0 {
                                span { class: "shrink-0 text-xs tabular-nums", "{n}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
