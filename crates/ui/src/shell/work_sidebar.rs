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

    use architect_ui::lucide_dioxus as i;
    let to_process = crate::process::use_to_process_count();
    let on_inbox = matches!(current, Route::InboxRoute {});
    let favorites = use_context::<crate::favorites::Favorites>().0;
    let mut projects_open = use_signal(|| true);
    let mut favorites_open = use_signal(|| true);

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

    let smart = [
        (TaskSection::Today, c.today, rsx! { i::CalendarCheck {} }),
        (
            TaskSection::Upcoming,
            c.upcoming,
            rsx! { i::CalendarDays {} },
        ),
        (TaskSection::Anytime, c.anytime, rsx! { i::Layers {} }),
    ];

    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col",
            // ── The workspace ──
            div { class: "px-3 pb-2 pt-3",
                crate::shell::org_switcher::OrgSwitcher { compact: false, theme: false }
            }
            nav { class: "flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-2 pb-4 pt-1", aria_label: "Work",
                // ── Your work ──
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
                    for (section, n, glyph) in smart {
                        {
                            let here = on_tasks && in_view.read().as_deref() == Some(section.anchor());
                            rsx! {
                                button {
                                    key: "{section.label()}",
                                    r#type: "button",
                                    aria_current: if here { "true" } else { "false" },
                                    class: row(here),
                                    onclick: move |_| {
                                        if !on_tasks {
                                            nav.push(Route::TasksRoute {});
                                        }
                                        scroll_to(section.anchor(), false);
                                    },
                                    span { class: icon, {glyph} }
                                    span { "{section.label()}" }
                                    span { class: count,
                                        if section == TaskSection::Today && c.overdue > 0 {
                                            span { class: "mr-1.5 rounded-full bg-rose-500/20 px-1.5 py-0.5 text-[11px] font-medium text-rose-300",
                                                "{c.overdue} late"
                                            }
                                        }
                                        if n > 0 { "{n}" }
                                    }
                                }
                            }
                        }
                    }
                    if c.unfiled > 0 {
                        button {
                            r#type: "button",
                            class: row(false),
                            onclick: move |_| {
                                if !on_tasks {
                                    nav.push(Route::TasksRoute {});
                                }
                                scroll_to("tasks-unfiled", true);
                            },
                            span { class: icon, i::CircleDashed {} }
                            span { "No project" }
                            span { class: count, "{c.unfiled}" }
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
                div { class: "flex flex-col gap-0.5",
                    div { class: "flex items-center",
                        button {
                            r#type: "button",
                            class: heading,
                            aria_expanded: "{projects_open}",
                            onclick: move |_| projects_open.toggle(),
                            "Projects"
                            span { class: if projects_open() { "transition-transform rotate-90" } else { "transition-transform" },
                                i::ChevronRight { size: 12 }
                            }
                        }
                        Link {
                            to: Route::ProjectsRoute {},
                            class: "shrink-0 px-2.5 pb-1 text-[12px] text-muted-foreground hover:text-foreground",
                            "All"
                        }
                    }
                    if projects_open() {
                        if c.projects.is_empty() {
                            p { class: "px-2.5 text-[13px] text-muted-foreground", "No active projects." }
                        }
                        for (pid, title, n) in c.projects.iter().cloned() {
                            {
                                let here = matches!(&current, Route::ProjectDetailRoute { id } if *id == pid);
                                rsx! {
                                    Link {
                                        key: "{pid}",
                                        to: Route::ProjectDetailRoute { id: pid.clone() },
                                        class: row(here),
                                        img {
                                            class: "size-5 shrink-0 rounded-[5px] object-cover",
                                            src: "{task_ui_core::abstract_art::data_url(&pid)}",
                                            alt: "",
                                        }
                                        span { class: "min-w-0 truncate", "{title}" }
                                        if n > 0 { span { class: count, "{n}" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            AccountFooter {}
        }
    }
}

/// The sidebar's foot: who is signed in, their status, and where this
/// device's data lives. Opens the account sheet on a phone and Settings
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
    let mut sheet = use_signal(|| false);
    let nav = use_navigator();
    rsx! {
        div { class: "border-t border-border/60 p-2 pb-[max(0.5rem,env(safe-area-inset-bottom,0px))] md:pb-2",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-3 rounded-[10px] px-2 py-2 text-left hover:bg-foreground/[0.05] active:bg-foreground/[0.08]",
                onclick: move |_| {
                    if phone {
                        sheet.set(true);
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
        task_ui_core::sheet::BottomSheet {
            open: sheet(),
            title: "Account & status".to_string(),
            on_close: move |()| sheet.set(false),
            crate::auth::AccountSheetBody { on_done: move |()| sheet.set(false) }
        }
    }
}
