//! `/tasks` — live task workspace over the org's `TaskService`.
//!
//! Thin page: the shared task store ([`crate::stores`]) loads the
//! selected orgs' tasks as one `AtomResult` (slug-tagged rows, so
//! "All" mode routes each edit back to its owning org), renders
//! [`task_ui::TasksApp`] through the shared forward converter, and
//! applies board mutations through the optimistic
//! [`crate::stores::TaskMutations`] (instant patch, reconcile or
//! rollback + tray notification).
//!
//! Defaults to **Active + Relevant**:
//! the filtering itself is `task_proto::relevance` — the same domain
//! functions the server's `TaskService::query` and the CLI's
//! `task task list --relevant` run — this page only assembles the
//! [`task_proto::RelevanceContext`] (browser clock + the running timer
//! session's project) and renders toggle chips. Toggles persist
//! per-account ([`crate::prefs`]).
//!
//! The page opens with the **Now bar**: the in-progress task, its
//! live clock, and a complete button — the checkbox-timer made
//! spatial. Idle (nothing running) renders no bar at all.

use architect_ui::lucide_dioxus::{Plus, SlidersHorizontal};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use task_ui::{QuickAdd, TaskInfo as UiTask, TaskMutation, TasksApp};

use crate::chrome::{
    PausedTimer, fmt_hms, owner_id, req_from_session, resolve_org, use_resume_hint, use_second_tick,
};
use crate::orgs::{OrgMeta, OrgSelection};
use crate::prefs::PrefsCtx;
use crate::shell::mobile::{BottomSheet, MobileActionBar};
use crate::stores;

/// The "I'm at:" choices — matched against `@<location>` gate
/// contexts by the relevance rules. FUTURE: personal named
/// locations on the prefs entity.
const LOCATIONS: &[&str] = &["home", "studio", "out"];

/// Everything the loaded board renders, derived once per **data**
/// change (tasks / projects / sessions / prefs) instead of on every
/// render of [`TasksView`] — the filter → condense → sort → convert
/// pipeline walks and clones the whole task list.
#[derive(Clone, PartialEq)]
struct BoardData {
    ui_tasks: Vec<UiTask>,
    /// Non-project belonging per task id — the chip that keeps a
    /// project-less row from reading as a bare title.
    anchors: Vec<(uuid::Uuid, task_ui::AnchorChip)>,
    /// Open tasks anchored to nothing at all. Not mixed into the
    /// list: they render in the triage strip, where the only useful
    /// action is filing them.
    triage: Vec<UiTask>,
    /// `(id, title)` of every known project — chip resolution + the
    /// quick-add `[[Project]]` picker.
    projects: Vec<(uuid::Uuid, String)>,
    /// Rows the Active/Relevant chips filtered out.
    hidden: usize,
    /// What those are, for the header ("3 done hidden").
    hidden_note: Option<String>,
    /// `(count, contexts)` the Relevant filter holds back — the list
    /// says so at its foot, with a way to show them.
    held: (usize, Vec<String>),
    /// `(task id, org)` when the board spans several orgs.
    org_of: Vec<(uuid::Uuid, String)>,
    /// The running work session (the unified timer source), if any —
    /// what the Now bar reflects. `None` when nothing is tracking.
    now: Option<NowInfo>,
}

/// The Now bar's data, derived from the open [`WorkSession`] rather
/// than task markdown — so the banner, the top-bar widget, and `/timer`
/// all show the same running timer. `task_id` is resolved from the
/// session's `task_note_path` (present only for task-linked timers) and
/// gates the Complete button.
#[derive(Clone, PartialEq)]
struct NowInfo {
    title: String,
    start_time: chrono::DateTime<chrono::Utc>,
    slug: String,
    org_id: uuid::Uuid,
    session_id: uuid::Uuid,
    task_id: Option<uuid::Uuid>,
    /// Rebuilt start request, so Pause can stash it for one-click resume.
    req: timer_proto::StartTimerRequest,
}

#[component]
pub fn TasksView() -> Element {
    let nav = use_navigator();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let prefs_ctx = use_context::<PrefsCtx>();

    // Mobile sheets: filters + quick-add, opened from the sticky
    // bottom action bar (thumb reach) below `md`.
    let mut filters_open = use_signal(|| false);
    let mut add_open = use_signal(|| false);

    // The list hooks drive the fetches; the derivation below reads the
    // backing stores directly (inside the memo) so it re-runs on data
    // changes only.
    let result = stores::use_task_list();
    let muts = stores::use_task_mutations();
    let store = stores::use_task_store();
    let _sessions = stores::use_session_list();
    let _projects = stores::use_project_list();
    let session_store = stores::use_session_store();
    let project_store = stores::use_project_store();

    // Server-backed per-user prefs — reading the signal makes the
    // chips follow the account (switch users, keep your setup).
    let user_prefs = prefs_ctx.prefs.read().clone();
    let active_only = user_prefs.tasks_active;
    let relevant_only = user_prefs.tasks_relevant;
    let at_location = user_prefs.location.clone();
    // "I'm at" location picker — one String signal shared by the desktop
    // chip and the mobile sheet select; on_change writes through to prefs.
    let location_pick = use_signal(|| user_prefs.location.clone());

    let prefs_signal = prefs_ctx.prefs;
    let board = use_memo(move || -> Option<BoardData> {
        let result = store.entries_result();
        let rows = result.value()?;

        let prefs = prefs_signal.read();
        let active_only = prefs.tasks_active;
        let relevant_only = prefs.tasks_relevant;
        let at_location = prefs.location.clone();
        drop(prefs);

        // The running timer session's project — relevance boosts its
        // sibling tasks to the top ("if my timer is on a project,
        // prioritize that project's items").
        let active_project = session_store
            .list()
            .iter()
            .find(|r| r.session.end_time.is_none())
            .and_then(|r| r.session.project_id);

        let ctx = crate::task_board::relevance_ctx(&at_location, active_project);
        // One row per task id — see `stores::unique_tasks`.
        let unique = stores::unique_tasks(&rows);
        let all: Vec<&task_proto::TaskInfo> = unique.iter().map(|r| &r.task).collect();
        // The board's filters — the same function the Work sidebar
        // counts with, so the two agree (`crate::task_board`).
        let filtered = crate::task_board::filter(&all, active_only, relevant_only, &ctx);
        let hidden = filtered.hidden_done + filtered.hidden_elsewhere;
        let hidden_note = filtered.hidden_note();
        let held = (filtered.hidden_elsewhere, filtered.held_by.clone());
        let mut domain = filtered.visible;
        let index = crate::task_board::ProjectIndex::new(
            project_store
                .list()
                .iter()
                .map(|r| (r.project.id, r.project.title.clone())),
        );
        let project_names = &index.names;
        let resolve_anchor = |t: &task_proto::TaskInfo| index.anchor(t);

        // Unfiled work is also listed above the board, with a picker to
        // file it. They stay in the list too: a task due tomorrow is due
        // tomorrow whether or not it has a project yet, and hiding it
        // in a collapsed strip is how it gets missed.
        let mut triage: Vec<&task_proto::TaskInfo> = domain
            .iter()
            .copied()
            .filter(|t| index.is_unfiled(t))
            .collect();
        triage.sort_by(|a, b| {
            a.date_created
                .cmp(&b.date_created)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        // The org each task lives in, named only when the board spans
        // more than one — a team's tasks and a client's look alike
        // otherwise.
        let slugs: std::collections::HashSet<&str> =
            unique.iter().map(|r| r.slug.as_str()).collect();
        let org_of: Vec<(uuid::Uuid, String)> = if slugs.len() > 1 {
            let names: std::collections::HashMap<String, String> = org_list
                .peek()
                .iter()
                .map(|o| (o.slug.clone(), o.name.clone()))
                .collect();
            unique
                .iter()
                .map(|r| {
                    let label = names
                        .get(&r.slug)
                        .filter(|n| !n.is_empty())
                        .cloned()
                        .unwrap_or_else(|| r.slug.clone());
                    (r.task.id, label)
                })
                .collect()
        } else {
            Vec::new()
        };

        // Every remaining task is a row: the list sorts them into Today
        // / Upcoming / Anytime itself, and nothing folds out of sight —
        // folding to one "next action" per project is how overdue work
        // used to vanish behind an undated row.
        if relevant_only {
            domain.sort_by_key(|t| task_proto::relevance_rank(t, &ctx));
        }

        let to_ui = |t: &task_proto::TaskInfo| -> UiTask {
            let mut row = t.clone();
            if row.projects.is_empty() {
                if let Some(name) = t.project_id.and_then(|id| project_names.get(&id)) {
                    row.projects.push(name.clone());
                }
            }
            row
        };
        // Chips for rows whose belonging isn't a project. A parent
        // resolves to its title out of the full task list (the parent
        // itself is often filtered out of the board); workstreams and
        // milestones have no client store yet, so they name their
        // kind — less than ideal, still not nothing.
        let task_titles: std::collections::HashMap<uuid::Uuid, &str> =
            all.iter().map(|t| (t.id, t.title.as_str())).collect();
        let chip_for = |t: &task_proto::TaskInfo| -> Option<task_ui::AnchorChip> {
            let a = resolve_anchor(t)?;
            match a {
                // Projects already speak through the `projects` chip
                // that `to_ui` backfills.
                task_proto::Anchor::Project(_) => None,
                task_proto::Anchor::Parent(id) => Some(task_ui::AnchorChip {
                    kind: "parent".into(),
                    label: task_titles
                        .get(&id)
                        .map_or_else(|| "parent task".to_owned(), |s| (*s).to_owned()),
                }),
                task_proto::Anchor::Workstream(_) => Some(task_ui::AnchorChip {
                    kind: "workstream".into(),
                    label: "Workstream".into(),
                }),
                task_proto::Anchor::Milestone(_) => Some(task_ui::AnchorChip {
                    kind: "milestone".into(),
                    label: "Milestone".into(),
                }),
            }
        };
        let anchors: Vec<(uuid::Uuid, task_ui::AnchorChip)> = domain
            .iter()
            .filter_map(|t| chip_for(t).map(|c| (t.id, c)))
            .collect();

        let ui_tasks: Vec<UiTask> = domain.iter().map(|t| to_ui(t)).collect();
        let triage: Vec<UiTask> = triage.iter().map(|t| to_ui(t)).collect();

        // The Now bar reflects the open work session (the unified timer
        // source) for the active org + owner — not task markdown — so a
        // running clock started anywhere (widget, /timer, here) is never
        // invisible and never disagrees with the top-bar widget.
        let now: Option<NowInfo> =
            resolve_org(&selection.read(), &org_list.read()).and_then(|(slug, org_id)| {
                let owner = owner_id(org_id);
                session_store
                    .list()
                    .into_iter()
                    .find(|r| {
                        r.slug == slug && r.session.user_id == owner && r.session.end_time.is_none()
                    })
                    .map(|r| {
                        let s = &r.session;
                        let matched = (!s.task_note_path.is_empty())
                            .then(|| {
                                rows.iter()
                                    .map(|(_, t)| &t.task)
                                    .find(|t| t.path == s.task_note_path)
                            })
                            .flatten();
                        let title = if s.description.trim().is_empty() {
                            matched
                                .map(|t| t.title.clone())
                                .unwrap_or_else(|| "Tracking".to_string())
                        } else {
                            s.description.clone()
                        };
                        NowInfo {
                            title,
                            start_time: s.start_time,
                            slug: slug.clone(),
                            org_id,
                            session_id: s.id,
                            task_id: matched.map(|t| t.id),
                            req: req_from_session(org_id, s),
                        }
                    })
            });

        Some(BoardData {
            ui_tasks,
            anchors,
            triage,
            projects: project_names
                .iter()
                .map(|(id, t)| (*id, t.clone()))
                .collect(),
            hidden,
            hidden_note,
            held,
            org_of,
            now,
        })
    });

    // The people a task can be given to: the members of the org new
    // work lands in.
    let target_org =
        use_memo(move || crate::orgs::create_target(&selection.read(), &org_list.read()));
    let member_names = crate::pages::wiki_access::use_member_names(target_org);
    let people = move || -> Vec<(String, String)> {
        let mut list: Vec<(String, String)> = member_names
            .read()
            .iter()
            .map(|(id, name)| (id.clone(), name.clone()))
            .collect();
        list.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        list
    };

    // Every board mutation goes through here: completing, dropping or
    // deleting a task raises a toast with Undo. The Undo closure holds
    // only app-lifetime handles (the store), so it still works after
    // the row — or this page — is gone.
    let toast = use_toast();
    let apply_with_undo = move |mu: TaskMutation| {
        let create_slug = crate::orgs::create_target(&selection.read(), &org_list.read());
        crate::task_board::apply_with_undo(muts, toast, &create_slug, mu);
    };

    let body = match (board(), result.error()) {
        (Some(data), _) => {
            let BoardData {
                ui_tasks,
                anchors,
                triage,
                projects: project_choices,
                hidden,
                hidden_note,
                held: (held_count, held_by),
                org_of,
                now,
            } = data;
            // Inline chips are desktop chrome; on phones the same
            // controls live in the Filters bottom sheet below.
            let chips = rsx! {
                div { class: "hidden items-center gap-1.5 md:flex",
                    FilterChip {
                        label: "Active",
                        title: "Hide done/cancelled tasks",
                        on: active_only,
                        on_toggle: move |on| prefs_ctx.update(|p| p.tasks_active = on),
                    }
                    FilterChip {
                        label: "Relevant",
                        title: "Only what matters right now — routines in their time windows, deadlines always, timer project first",
                        on: relevant_only,
                        on_toggle: move |on| prefs_ctx.update(|p| p.tasks_relevant = on),
                    }
                    // "I'm at:" — feeds the @location relevance gates
                    // and persists per user. Labelled, so it reads as a
                    // setting rather than a stray dropdown. (What the
                    // filters hide is said once, in the header line.)
                    span { class: "ml-1 whitespace-nowrap text-xs text-muted-foreground", "I'm at" }
                    Select {
                        value: location_pick,
                        placeholder: "anywhere".to_string(),
                        on_change: move |v: String| prefs_ctx.update(|p| p.location = v.clone()),
                        SelectContent {
                            for (i, loc) in LOCATIONS.iter().enumerate() {
                                SelectItem { key: "{loc}", value: "{loc}", index: i, "@{loc}" }
                            }
                        }
                    }
                }
            };
            // How many filters are narrowing the board — badged on the
            // mobile Filters trigger so a filtered board is never a
            // mystery on a phone.
            let filter_count = usize::from(active_only)
                + usize::from(relevant_only)
                + usize::from(!at_location.is_empty());
            let sheet_projects = project_choices.clone();
            rsx! {
                div { class: "flex h-full w-full flex-col pb-14 md:pb-0",
                    if let Some(n) = now {
                        NowBar {
                            now: n,
                            on_complete: move |id| {
                                apply_with_undo(TaskMutation::SetStatus { id, status: "done".into() });
                            },
                        }
                    }
                    div { class: "min-h-0 flex-1",
                        TasksApp {
                            tasks: ui_tasks,
                            anchors,
                            triage,
                            header_extra: chips,
                            projects: project_choices,
                            org_of,
                            hidden_note,
                            held_count,
                            held_by,
                            on_show_held: move |()| prefs_ctx.update(|p| p.tasks_relevant = false),
                            people: people(),
                            initial_view: stored_view(),
                            on_view_change: move |v| remember_view(v),
                            on_event: move |mu: TaskMutation| apply_with_undo(mu),
                            on_open_full: move |id| {
                                nav.push(crate::routes::Route::TaskDetailRoute { id });
                            },
                        }
                    }
                    // ── Mobile: sticky actions + sheets ────────────────
                    MobileActionBar {
                        button {
                            r#type: "button",
                            class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg border border-border px-3 py-2 text-sm font-medium text-foreground active:bg-accent",
                            onclick: move |_| filters_open.set(true),
                            SlidersHorizontal { size: 16 }
                            if filter_count > 0 { "Filters · {filter_count}" } else { "Filters" }
                        }
                        button {
                            r#type: "button",
                            class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg bg-primary px-3 py-2 text-sm font-medium text-primary-foreground active:bg-primary/85",
                            onclick: move |_| add_open.set(true),
                            Plus { size: 16 }
                            "Add task"
                        }
                    }
                    BottomSheet {
                        open: filters_open(),
                        title: "Filters".to_string(),
                        on_close: move |()| filters_open.set(false),
                        div { class: "flex flex-col gap-3 pb-2",
                            SheetToggleRow {
                                label: "Active",
                                hint: "Hide done and cancelled tasks",
                                on: active_only,
                                on_toggle: move |on| prefs_ctx.update(|p| p.tasks_active = on),
                            }
                            SheetToggleRow {
                                label: "Relevant",
                                hint: "Only what matters right now",
                                on: relevant_only,
                                on_toggle: move |on| prefs_ctx.update(|p| p.tasks_relevant = on),
                            }
                            div { class: "flex flex-col gap-1.5",
                                span { class: "px-1 text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                                    "I'm at"
                                }
                                Select {
                                    value: location_pick,
                                    placeholder: "anywhere".to_string(),
                                    class: "min-h-11 w-full".to_string(),
                                    on_change: move |v: String| prefs_ctx.update(|p| p.location = v.clone()),
                                    SelectContent {
                                        for (i, loc) in LOCATIONS.iter().enumerate() {
                                            SelectItem { key: "{loc}", value: "{loc}", index: i, "@{loc}" }
                                        }
                                    }
                                }
                            }
                            if hidden > 0 {
                                span { class: "px-1 text-xs tabular-nums text-muted-foreground",
                                    "{hidden} tasks hidden by these filters"
                                }
                            }
                        }
                    }
                    BottomSheet {
                        open: add_open(),
                        title: "Add task".to_string(),
                        on_close: move |()| add_open.set(false),
                        div { class: "pb-2",
                            QuickAdd {
                                projects: sheet_projects,
                                // The sheet opened to type: the keyboard
                                // comes up with it.
                                autofocus: true,
                                on_create: move |task: task_ui::TaskInfo| {
                                    let create_slug =
                                        crate::orgs::create_target(&selection.read(), &org_list.read());
                                    muts.apply(&create_slug, TaskMutation::Create { task });
                                    add_open.set(false);
                                },
                            }
                        }
                    }
                }
            }
        }
        (None, Some(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't reach the task service",
                message: e,
                on_retry: move |()| store.reload(),
            }
        },
        (None, None) => rsx! { crate::states::LoadingState {} },
    };

    rsx! {
        div { class: "h-full w-full", {body} }
    }
}

/// The page's opening statement: what's running right now. A slim
/// full-width strip — pulsing sky dot, timer title, live elapsed clock,
/// and the same Pause / Stop controls as the top-bar widget, plus
/// Complete when the timer is linked to a task. Driven by the open
/// [`WorkSession`], so it always agrees with the widget. Rendered only
/// while a clock is live, so an idle board stays clean.
#[component]
fn NowBar(now: NowInfo, on_complete: EventHandler<uuid::Uuid>) -> Element {
    let tick = use_signal(|| 0u64);
    use_second_tick(tick);
    let _ = tick(); // subscribe: the clock re-renders each second

    let muts = stores::use_timer_mutations();
    let mut hint = use_resume_hint();
    let elapsed = (chrono::Utc::now() - now.start_time).num_seconds();
    let owner = owner_id(now.org_id);

    let pause = {
        let now = now.clone();
        move |_| {
            muts.stop(now.slug.clone(), owner, now.session_id);
            hint.set(Some(PausedTimer {
                slug: now.slug.clone(),
                org_id: now.org_id,
                req: now.req.clone(),
                title: now.title.clone(),
                banked: elapsed.max(0),
            }));
        }
    };
    let stop = {
        let slug = now.slug.clone();
        let sid = now.session_id;
        move |_| {
            muts.stop(slug.clone(), owner, sid);
            hint.set(None);
        }
    };
    let complete = {
        let slug = now.slug.clone();
        let sid = now.session_id;
        let task_id = now.task_id;
        move |_| {
            // Landing the task also stops its clock and clears any pause.
            muts.stop(slug.clone(), owner, sid);
            hint.set(None);
            if let Some(id) = task_id {
                on_complete.call(id);
            }
        }
    };

    rsx! {
        div { class: "flex items-center gap-3 border-b border-sky-500/30 bg-sky-500/10 px-4 py-2 sm:px-6 lg:px-8",
            span { class: "relative flex size-2 shrink-0",
                span { class: "absolute inline-flex size-full animate-ping rounded-full bg-sky-400/70" }
                span { class: "relative inline-flex size-2 rounded-full bg-sky-400" }
            }
            span { class: "min-w-0 truncate text-sm font-medium text-foreground", "{now.title}" }
            span { class: "shrink-0 font-mono text-sm tabular-nums text-sky-300",
                "{fmt_hms(elapsed)}"
            }
            div { class: "ml-auto flex shrink-0 items-center gap-1.5",
                button {
                    r#type: "button",
                    class: "rounded-md border border-amber-500/40 px-2.5 py-0.5 text-xs font-medium text-amber-200 transition-colors hover:bg-amber-500/20",
                    onclick: pause,
                    "Pause"
                }
                button {
                    r#type: "button",
                    class: "rounded-md border border-border px-2.5 py-0.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground",
                    onclick: stop,
                    "Stop"
                }
                if now.task_id.is_some() {
                    button {
                        r#type: "button",
                        class: "rounded-md border border-sky-500/40 px-2.5 py-0.5 text-xs font-medium text-sky-200 transition-colors hover:bg-sky-500/20",
                        onclick: complete,
                        "Complete"
                    }
                }
            }
        }
    }
}

/// Touch-sized on/off row for the mobile Filters sheet — the same
/// toggles the desktop chips drive, at thumb size (≥44px), with a
/// switch-style state indicator.
#[component]
fn SheetToggleRow(
    label: &'static str,
    hint: &'static str,
    on: bool,
    on_toggle: EventHandler<bool>,
) -> Element {
    let (track, knob) = if on {
        ("bg-primary", "translate-x-4")
    } else {
        ("bg-muted", "translate-x-0")
    };
    rsx! {
        button {
            r#type: "button",
            class: "flex min-h-12 w-full items-center justify-between gap-3 rounded-lg border border-border/60 bg-card/40 px-3 py-2 text-left active:bg-accent",
            onclick: move |_| on_toggle.call(!on),
            div { class: "flex min-w-0 flex-col",
                span { class: "text-sm font-medium text-foreground", "{label}" }
                span { class: "text-xs text-muted-foreground", "{hint}" }
            }
            span { class: "relative h-5 w-9 shrink-0 rounded-full transition-colors {track}",
                span { class: "absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-background shadow transition-transform {knob}" }
            }
        }
    }
}

/// Small on/off filter chip.
#[component]
fn FilterChip(label: String, title: String, on: bool, on_toggle: EventHandler<bool>) -> Element {
    let cls = if on {
        "rounded-full border border-primary/50 bg-primary/10 px-2.5 py-0.5 text-xs font-medium text-primary transition-colors"
    } else {
        "rounded-full border border-border px-2.5 py-0.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
    };
    rsx! {
        button {
            r#type: "button",
            class: "{cls} focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            title: "{title}",
            aria_pressed: on.to_string(),
            onclick: move |_| on_toggle.call(!on),
            "{label}"
        }
    }
}

/// Where the List / Kanban choice is kept on this device.
#[cfg(target_arch = "wasm32")]
const VIEW_KEY: &str = "task.tasks.view";

/// The List / Kanban choice this device last made. Read synchronously:
/// the board takes its initial view once, at mount.
#[cfg(target_arch = "wasm32")]
fn stored_view() -> Option<task_ui::ViewMode> {
    let stored = web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(VIEW_KEY)
        .ok()??;
    match stored.as_str() {
        "kanban" => Some(task_ui::ViewMode::Kanban),
        "list" => Some(task_ui::ViewMode::List),
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
fn remember_view(view: task_ui::ViewMode) {
    let value = match view {
        task_ui::ViewMode::Kanban => "kanban",
        task_ui::ViewMode::List => "list",
    };
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(VIEW_KEY, value);
    }
}

/// Native shells start on the list (the webview's storage is not
/// reachable synchronously from here).
#[cfg(not(target_arch = "wasm32"))]
fn stored_view() -> Option<task_ui::ViewMode> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn remember_view(_view: task_ui::ViewMode) {}
