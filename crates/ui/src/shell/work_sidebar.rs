//! The work sidebar — what the explorer column shows wherever the
//! person is planning or doing work (Home, Tasks, Projects, Schedule,
//! Inbox, Timer, Goals, Milestones, Gantt).
//!
//! The vault tree is the right companion inside notes and wikis, and
//! the wrong one here: beside a task list it is a column of folders
//! that answers nothing being asked. This column answers "what is on
//! my plate": the task list's own sections with live counts, and every
//! active project with how much is open in it.
//!
//! Its counts come from the same rules as the list itself — the
//! person's filters ([`crate::task_board::filter`]), the same section
//! rule ([`task_ui::task_section`]) and the same "unfiled" test
//! ([`crate::task_board::ProjectIndex::is_unfiled`]) — so a sidebar
//! number and the list beneath it can't disagree.

use dioxus::prelude::*;
use task_ui::TaskSection;

use crate::routes::Route;
use crate::stores;
use crate::task_sort::is_active;

/// Scroll the Tasks page to an element, waiting for the list to render
/// when the click also navigated there. `open` also expands it when it
/// is a collapsed disclosure (the unfiled strip).
fn scroll_to(anchor: &'static str, open: bool) {
    let _ = dioxus::document::eval(&format!(
        "(function go(n){{const e=document.getElementById('{anchor}');\
         if(e){{e.scrollIntoView({{behavior:'smooth',block:'start'}});\
         if({open}){{const b=e.querySelector('button[aria-expanded=false]');if(b)b.click();}}}}\
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
    let _sessions = stores::use_session_list();
    let task_store = stores::use_task_store();
    let project_store = stores::use_project_store();
    let session_store = stores::use_session_store();
    let prefs = use_context::<crate::prefs::PrefsCtx>().prefs;
    let nav = use_navigator();
    // The same hour as the Tasks page, so its counts and these agree.
    let now_block = crate::now_block::use_now_block();

    let counts = use_memo(move || {
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
        let focus = now_block
            .read()
            .as_ref()
            .map(|b| b.focus.clone())
            .unwrap_or_default();
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
            upcoming: 0,
            anytime: 0,
            unfiled: 0,
            projects: Vec::new(),
        };
        for t in &board {
            // Unfiled work is counted in its section like everything
            // else, and once more here — the number to drive to zero.
            if index.is_unfiled(t) {
                c.unfiled += 1;
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
    // Which list section is on screen — the sidebar marks it, so the
    // column says where you are, not just where you could go.
    let mut in_view = use_signal(|| None::<String>);
    use_effect(move || {
        if !on_tasks {
            return;
        }
        spawn(async move {
            let mut watch = dioxus::document::eval(
                "const ids=['tasks-today','tasks-upcoming','tasks-anytime'];\
                 let last='';\
                 const pick=()=>{let cur='';for(const id of ids){const e=document.getElementById(id);\
                   if(e&&e.getBoundingClientRect().top<180)cur=id;}\
                   if(!cur){for(const id of ids){if(document.getElementById(id)){cur=id;break;}}}\
                   if(cur!==last){last=cur;dioxus.send(cur);}};\
                 document.addEventListener('scroll',pick,{capture:true,passive:true});\
                 setInterval(pick,800);pick();\
                 await new Promise(()=>{});",
            );
            while let Ok(id) = watch.recv::<String>().await {
                in_view.set((!id.is_empty()).then_some(id));
            }
        });
    });

    let smart = [
        (TaskSection::Today, c.today),
        (TaskSection::Upcoming, c.upcoming),
        (TaskSection::Anytime, c.anytime),
    ];
    let item = "flex items-center justify-between rounded-md px-2 py-1.5 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

    rsx! {
        nav { class: "flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-3", aria_label: "Work",
            div { class: "flex flex-col gap-0.5",
                for (section, n) in smart {
                    {
                        let here = on_tasks && in_view.read().as_deref() == Some(section.anchor());
                        let empty = n == 0;
                        rsx! {
                            button {
                                key: "{section.label()}",
                                r#type: "button",
                                disabled: empty,
                                aria_current: if here { "true" } else { "false" },
                                class: if here {
                                    "{item} bg-accent font-medium text-foreground"
                                } else if empty {
                                    "{item} cursor-default text-muted-foreground"
                                } else {
                                    "{item} text-foreground/90 hover:bg-accent/40"
                                },
                                onclick: move |_| {
                                    if !on_tasks {
                                        nav.push(Route::TasksRoute {});
                                    }
                                    scroll_to(section.anchor(), false);
                                },
                                span { "{section.label()}" }
                                span { class: "flex items-center gap-1.5 text-xs tabular-nums text-muted-foreground",
                                    if section == TaskSection::Today && c.overdue > 0 {
                                        span { class: "rounded-full bg-rose-500/20 px-1.5 py-0.5 font-medium text-foreground",
                                            "{c.overdue} late"
                                        }
                                    }
                                    "{n}"
                                }
                            }
                        }
                    }
                }
                if c.unfiled > 0 {
                    button {
                        r#type: "button",
                        class: "{item} text-foreground/90 hover:bg-accent/40",
                        onclick: move |_| {
                            if !on_tasks {
                                nav.push(Route::TasksRoute {});
                            }
                            scroll_to("tasks-unfiled", true);
                        },
                        span { "No project" }
                        span { class: "text-xs tabular-nums text-muted-foreground", "{c.unfiled}" }
                    }
                }
            }

            div { class: "flex flex-col gap-1",
                div { class: "flex items-center justify-between px-2",
                    span { class: "text-xs font-medium text-muted-foreground", "Projects" }
                    Link {
                        to: Route::ProjectsRoute {},
                        class: "rounded-sm text-xs text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        "All projects"
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
                            class: "flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-sm text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
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
