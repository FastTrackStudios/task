//! `/projects/:id` — project overview.
//!
//! Resolves one project through the shared optimistic project store
//! ([`crate::stores::use_project`] — instant from cache after a
//! `/projects` visit, else a per-org probe), filters the shared task
//! store's rows to this project (by `project_id` or `[[wikilink]]`),
//! and renders the work full width: masthead, parts, the task board
//! ([`task_ui::TasksApp`], embedded). Reference material — details,
//! budget, who's active — rides the right panel behind the top bar's
//! toggle ([`details_panel::DetailsPanel`]).
//!
//! One file per concern: [`parts`] the tracklist, [`budget`] the money
//! and time math, [`active`] live activity, [`details_panel`] the
//! reference column. This module is the page's wiring and masthead.

mod active;
mod budget;
mod deliverable;
mod details_panel;
mod parts;

use agent_proto::session::{Session as AgentSession, SessionStatus as AgentStatus};
use architect_ui::prelude::*;
use chrono::Utc;
use dioxus::prelude::*;
use task_proto::TaskInfo as DbTask;
use task_ui::{TaskInfo as UiTask, TaskMutation};
use timer_proto::WorkSession;
use uuid::Uuid;

use crate::orgs::OrgMeta;
use crate::shell::mobile::{BottomSheet, MobileActionBar};
use crate::stores;
use crate::task_sort::{belongs, is_active_task};

use active::sleep_30s;
use budget::{budget_tile_value, build_budget};
use details_panel::DetailsPanel;
use parts::PartsSection;

#[component]
pub fn ProjectDetailView(id: String) -> Element {
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    // The shell's right-panel toggle (top bar) shows/hides this page's
    // reference column: details, budget, who's on it now.
    let right_open = use_context::<Signal<crate::chrome::RightPanelOpen>>();
    // Mobile add-task sheet — TasksApp hides its inline quick-add on
    // phones; the sticky action bar opens this instead.
    let mut add_open = use_signal(|| false);
    // The main column's one switch: the work itself, or its files.
    let mut tab = use_signal(|| MainTab::Overview);

    // The project itself: cache-first from the shared store (instant
    // after a /projects visit), else a per-org probe. Mutations
    // reconcile straight into this value — no page refresh counter.
    let project_res = stores::use_project(id.clone());
    let project_store = stores::use_project_store();

    // `(owning slug, project id)` once resolved — the key every
    // dependent fetch hangs off. Mirrored into a reactive memo so the
    // resources/hooks below re-run when it lands or changes.
    let pkey: Option<(String, Uuid)> = project_res.value().map(|r| (r.slug.clone(), r.project.id));
    let pkey_for_memo = pkey.clone();
    let pkey_memo = use_memo(use_reactive!(|(pkey_for_memo,)| pkey_for_memo));

    // The shared task store: the selected orgs' tasks (slug-tagged);
    // `belongs()` filters to this project at render time.
    let tasks_res = stores::use_task_list();
    let task_muts = stores::use_task_mutations();

    // Review feedback becomes work with one click: the comment rail's
    // "file as task" action (provided here because this page knows the
    // project the task belongs to). Title-only on purpose — `capture`
    // parses the title, and the file + timecode ride along as plain
    // words a human can read on the board.
    use_context_provider(|| {
        files_ui::review::ReviewTaskHook(EventHandler::new(
            move |req: files_ui::review::ReviewTaskRequest| {
                let Some((slug, pid)) = pkey_memo.peek().clone() else {
                    return;
                };
                let file = req.path.rsplit('/').next().unwrap_or(&req.path).to_owned();
                let mut title = req.body.trim().chars().take(120).collect::<String>();
                if title.is_empty() {
                    title = format!("Address {}'s note", req.author);
                }
                let secs = req.timecode_secs;
                if secs >= 0.0 {
                    let (m, s) = ((secs as u32) / 60, (secs as u32) % 60);
                    title.push_str(&format!(" · {file} {m}:{s:02}"));
                } else {
                    title.push_str(&format!(" · {file}"));
                }
                let task = UiTask::new(title);
                task_muts
                    .scoped_to(pid)
                    .apply(&slug, TaskMutation::Create { task });
            },
        ))
    });

    // The project's File Root, by the naming convention (the root is
    // named after the project — what the seeder and the adopt flow
    // both produce). Present → the Files tab mounts the FULL root
    // explorer (inspector, versions, review); absent → the org-tree
    // fallback, which can only show the project's vault notes.
    // Mirrored through `use_reactive` for the same reason `pkey_memo`
    // is: the store read is render-time, not reactive inside a memo.
    let tkey: Option<(String, String)> = project_res
        .value()
        .map(|r| (r.slug.clone(), r.project.title.clone()));
    let title_key = use_memo(use_reactive!(|(tkey,)| tkey));
    let project_root = use_resource(move || {
        let k = title_key();
        // Re-resolve on every switch INTO the tab: a lookup that ran
        // during page load can lose to a client still re-establishing,
        // and a sticky `None` would wedge the tab on the fallback.
        let _ = tab();
        async move {
            let (slug, title) = k?;
            files_ui::root_named(&slug, &title).await
        }
    });

    // Connected repos — read-only, fetched once the project resolves.
    let repos_res = use_resource(move || {
        let k = pkey_memo();
        async move {
            match k {
                Some((slug, pid)) => crate::feeds::fetch_repos_for_project(&slug, pid)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
    });

    // ── Live aux data: budget + active-now ──────────────────────────
    // Poll-refresh every 30s — a presence channel replaces this poll
    // in a follow-up issue; until then the tick re-runs both resources
    // below (native parks: see `active::sleep_30s`).
    let mut live_tick = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            sleep_30s().await;
            live_tick += 1;
        }
    });

    // Budget aggregates: every session logged against the project
    // (time spend), the org's uninvoiced groups (unbilled money), and
    // the invoice list (invoiced / paid via the `session.invoice_id`
    // join — one extra call, resolved in [`budget::build_budget`]).
    let budget = use_resource(move || {
        let _ = live_tick.read();
        let k = pkey_memo();
        async move {
            let (slug, pid) = k?;
            let (sessions, uninvoiced, invoices) = Box::pin(futures_util::future::join3(
                crate::feeds::fetch_project_sessions(&slug, pid, false),
                crate::feeds::fetch_uninvoiced(&slug),
                crate::feeds::fetch_invoices(&slug),
            ))
            .await;
            // Each piece is independently non-fatal: a finance hiccup
            // shouldn't hide the time budget (and vice versa).
            let sessions = sessions.unwrap_or_default();
            let uninvoiced = uninvoiced.unwrap_or_default();
            let invoices = invoices.unwrap_or_default();
            Some(build_budget(
                pid,
                &sessions,
                &uninvoiced,
                &invoices,
                Utc::now(),
            ))
        }
    });

    // Who's on the project right now: open timer sessions + agent
    // sessions mid-turn (Running) or blocked on a human (AwaitingUser).
    let active_now = use_resource(move || {
        let _ = live_tick.read();
        let k = pkey_memo();
        async move {
            let (slug, pid) = k?;
            let (timers, agents) = Box::pin(futures_util::future::join(
                crate::feeds::fetch_project_sessions(&slug, pid, true),
                crate::feeds::fetch_project_agent_sessions(&slug, &pid.to_string()),
            ))
            .await;
            let timers = timers.unwrap_or_default();
            let mut agents = agents.unwrap_or_default();
            agents.retain(|s| matches!(s.status, AgentStatus::Running | AgentStatus::AwaitingUser));
            Some((timers, agents))
        }
    });

    let body = match (project_res.value(), project_res.error()) {
        (Some(op), _) => {
            let p = op.project.clone();
            let forge_slug = op.slug.clone();
            let connected_repos: Vec<git_proto::RepoId> =
                repos_res.read().clone().unwrap_or_default();
            let all: Vec<DbTask> = tasks_res
                .value()
                .map(|rows| rows.iter().map(|(_, r)| r.task.clone()).collect())
                .unwrap_or_default();
            let mine: Vec<UiTask> = all.iter().filter(|t| belongs(t, &p)).cloned().collect();
            let total = mine.len();
            let done = mine.iter().filter(|t| t.status == "done").count();
            let pct: f32 = if p.progress_percent >= 0 {
                f32::from(p.progress_percent)
            } else if total > 0 {
                (done as f32 / total as f32) * 100.0
            } else {
                0.0
            };
            let kind = ProjectKind::from_str(&p.project_type);

            // In-flight slice of the project's tasks: in-progress, or
            // claimed (someone in `workflow.assignees`). Same
            // `belongs()` membership as the full board below.
            let active_tasks: Vec<DbTask> = all
                .iter()
                .filter(|t| belongs(t, &p) && is_active_task(t))
                .cloned()
                .collect();

            // Budget + active-now snapshots; `None` while loading so
            // the sections render quiet placeholders, never blanking.
            let bd: Option<budget::BudgetData> =
                budget.read_unchecked().as_ref().cloned().flatten();
            let active_snapshot: Option<(Vec<WorkSession>, Vec<AgentSession>)> =
                active_now.read_unchecked().as_ref().cloned().flatten();
            let budget_value = if p.estimated_seconds == 0 {
                "—".to_string()
            } else {
                bd.as_ref().map_or_else(
                    || "…".to_string(),
                    |b| budget_tile_value(b.logged_seconds, p.estimated_seconds),
                )
            };
            // Single-user stand-in identity (matches the timer chrome)
            // so "you" labels your own open session.
            let you: Option<Uuid> = org_list
                .read()
                .iter()
                .find(|o| o.slug == forge_slug)
                .and_then(|o| o.id)
                .map(crate::chrome::owner_id);
            rsx! {
                // ── Cover banner — the `image:` frontmatter, same source
                // the /projects cards render. Set/edited from the right
                // panel's Details card.
                if !p.image.trim().is_empty() {
                    div { class: "aspect-[3/1] w-full overflow-hidden rounded-2xl border border-border bg-muted",
                        img {
                            src: "{p.image}",
                            alt: "{p.title}",
                            loading: "lazy",
                            class: "h-full w-full object-cover",
                        }
                    }
                }

                // ── Masthead ────────────────────────────────────────────
                //
                // The eyebrow is the project's own vocabulary — what the
                // model actually declares (`form`, `capabilities`) — set
                // small-caps over the title, the way a record sleeve
                // credits the format before the name. Status is a dot in
                // the meta line, not a pill fighting the eyebrow; and
                // there is NO progress bar until there are tasks — an
                // empty full-width bar is a statement about nothing.
                // No bottom border of its own — the tab strip right
                // under it IS the masthead's rule; two hairlines four
                // pixels apart read as a rendering bug.
                div { class: "flex flex-col gap-1.5 pb-1",
                    div { class: "flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[11px] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                        {
                            let mut tokens: Vec<(String, bool)> = Vec::new();
                            if let Some(form) = p.form {
                                tokens.push((form.as_str().to_owned(), true));
                            }
                            for cap in p.capabilities.held.iter() {
                                tokens.push((cap.as_str().to_owned(), false));
                            }
                            rsx! {
                                for (i, (token, is_form)) in tokens.iter().enumerate() {
                                    span { key: "{token}",
                                        if i > 0 {
                                            span { class: "pr-1.5 text-border", "·" }
                                        }
                                        span {
                                            class: if *is_form { "text-primary" } else { "" },
                                            "{token}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "flex flex-wrap items-end justify-between gap-x-6 gap-y-2",
                        div { class: "flex min-w-0 items-center gap-3",
                            if !p.color.is_empty() {
                                span { class: "h-3.5 w-3.5 shrink-0 rounded-full", style: "background:{p.color}" }
                            }
                            Heading { level: HeadingLevel::H1, class: "min-w-0 break-words text-3xl tracking-tight", "{p.title}" }
                        }
                        // Who's here, quietly at the header's right edge.
                        crate::presence::ProjectPresenceStrip { project_id: p.id }
                    }
                    div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 pt-0.5 text-sm text-muted-foreground",
                        span { class: "inline-flex items-center gap-1.5",
                            span { class: "h-1.5 w-1.5 rounded-full {status_dot(&p.status)}" }
                            "{p.status}"
                        }
                        if !p.parts.0.is_empty() {
                            span { "·" }
                            span { class: "tabular-nums", "{p.parts.0.len()} parts" }
                        }
                        if total > 0 {
                            span { "·" }
                            // Progress lives IN the meta line: a short
                            // inline bar beside its own numbers, not a
                            // page-wide ruler with a floating percent.
                            span { class: "inline-flex items-center gap-2",
                                span { class: "h-1 w-16 overflow-hidden rounded-full bg-muted",
                                    span {
                                        class: "block h-full rounded-full bg-primary",
                                        style: "width: {pct}%",
                                    }
                                }
                                span { class: "tabular-nums", "{done}/{total} tasks" }
                            }
                        }
                        if p.estimated_seconds > 0 {
                            span { "·" }
                            span { class: "tabular-nums", "{budget_value}" }
                        }
                        if !p.lead.is_empty() {
                            span { "·" }
                            span { "lead {p.lead}" }
                        }
                        if let Some(d) = p.target_date {
                            span { "·" }
                            span { "due {d}" }
                        }
                    }
                    if !p.tags.0.is_empty() {
                        div { class: "flex flex-wrap gap-1.5 pt-1",
                            // Index-qualified key: frontmatter tags can repeat.
                            for (i, tag) in p.tags.0.iter().enumerate() {
                                span {
                                    key: "{i}-{tag}",
                                    class: "rounded-full border border-border bg-muted/60 px-2 py-0.5 text-[11px] text-muted-foreground",
                                    "{tag}"
                                }
                            }
                        }
                    }
                }

                // ── Body: the work, full width; reference material and
                // the task board ride the right panel (the top bar's
                // toggle). The main column is what the project IS: its
                // masters, its parts, its files.
                div { class: "flex min-w-0 items-start gap-6",
                    div { class: "flex min-w-0 flex-1 flex-col gap-6",
                        // Overview | Files — one quiet switch.
                        div { class: "flex items-center gap-1 border-b border-border/60",
                            for (label, t) in [("Overview", MainTab::Overview), ("Files", MainTab::Files)] {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    class: if tab() == t {
                                        "border-b-2 border-primary px-2 pb-1.5 text-sm font-medium text-foreground"
                                    } else {
                                        "border-b-2 border-transparent px-2 pb-1.5 text-sm text-muted-foreground hover:text-foreground"
                                    },
                                    onclick: move |_| tab.set(t),
                                    "{label}"
                                }
                            }
                        }
                        match tab() {
                            MainTab::Overview => rsx! {
                                // The main deliverable — the thing you came
                                // for. An album's master audio plays here; a
                                // documentary's cut opens with its first
                                // frame as the thumbnail.
                                deliverable::MasterDeliverables {
                                    project: p.clone(),
                                    slug: forge_slug.clone(),
                                }
                                PartsSection {
                                    project: p.clone(),
                                    slug: forge_slug.clone(),
                                }
                                // Code projects also carry their issues & PRs.
                                if kind == ProjectKind::Code && !connected_repos.is_empty() {
                                    div { class: "flex flex-col gap-2",
                                        Heading { level: HeadingLevel::H2, "Issues & Pull requests" }
                                        for rid in connected_repos.iter() {
                                            crate::forge_views::ForgePanel {
                                                key: "{rid.owner}/{rid.repo}",
                                                slug: forge_slug.clone(),
                                                repo_id: rid.clone(),
                                            }
                                        }
                                    }
                                }
                            },
                            MainTab::Files => rsx! {
                                // The project's actual files: its root's
                                // live tree through the full explorer
                                // (inspector, versions, review), when the
                                // root is adopted. The org-tree fallback
                                // covers unadopted projects — it can only
                                // show vault notes, which is exactly the
                                // honest picture then.
                                div { class: "min-h-[24rem]",
                                    match &*project_root.read_unchecked() {
                                        Some(Some(root)) => rsx! {
                                            files_ui::Explorer {
                                                org: forge_slug.clone(),
                                                start: files_ui::Location::Root {
                                                    id: root.id,
                                                    subpath: String::new(),
                                                },
                                                root: Some(root.clone()),
                                                embedded: true,
                                            }
                                        },
                                        Some(None) => rsx! {
                                            files_ui::tree::TreeExplorer {
                                                org: forge_slug.clone(),
                                                area: files_ui::tree::TreeArea::Projects,
                                                start: Some(format!("Projects/{}", p.title)),
                                            }
                                        },
                                        None => rsx! {
                                            span { class: "text-xs text-muted-foreground", "…" }
                                        },
                                    }
                                }
                            },
                        }
                    }
                    // ── Right panel: the task board + reference material,
                    // behind the top bar's toggle.
                    if right_open.read().0 {
                        DetailsPanel {
                            project: p.clone(),
                            slug: forge_slug.clone(),
                            snapshot: active_snapshot,
                            you,
                            tasks: mine.clone(),
                            active_tasks: active_tasks.clone(),
                            on_task: {
                                let create_slug = forge_slug.clone();
                                let scoped = task_muts.scoped_to(p.id);
                                move |mu: TaskMutation| {
                                    scoped.apply(&create_slug, mu);
                                }
                            },
                        }
                    }
                }

                // ── Mobile: sticky add-task above the tab bar ───────
                MobileActionBar {
                    button {
                        r#type: "button",
                        class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg bg-primary px-3 py-2 text-sm font-medium text-primary-foreground active:bg-primary/85",
                        onclick: move |_| add_open.set(true),
                        "Add task to {p.title}"
                    }
                }
                BottomSheet {
                    open: add_open(),
                    title: "Add task".to_string(),
                    on_close: move |()| add_open.set(false),
                    div { class: "pb-2",
                        task_ui::QuickAdd {
                            // Creates here file under THIS project — same
                            // scoped mutations the board uses.
                            on_create: {
                                let create_slug = forge_slug.clone();
                                let scoped = task_muts.scoped_to(p.id);
                                move |task: UiTask| {
                                    scoped.apply(&create_slug, TaskMutation::Create { task });
                                    add_open.set(false);
                                }
                            },
                        }
                    }
                }
            }
        }
        (None, Some(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't load project",
                message: e.clone(),
                on_retry: move |()| project_store.reload(),
            }
        },
        (None, None) => rsx! { crate::states::LoadingState {} },
    };

    rsx! {
        // Full width — the shell's sidebars already frame the page, and
        // a centered column inside them read as a page within a page.
        div { class: "flex w-full flex-col gap-6 p-4 pb-14 sm:p-6 md:pb-6 lg:px-8 lg:py-6", {body} }
    }
}

/// The main column's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MainTab {
    Overview,
    Files,
}

/// Project type → overview layout. Free-form string under the hood;
/// unknown / empty ⇒ General.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Code,
    General,
    Personal,
}

impl ProjectKind {
    fn from_str(s: &str) -> Self {
        match s {
            "code" => Self::Code,
            "personal" => Self::Personal,
            _ => Self::General,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::General => "General",
            Self::Personal => "Personal",
        }
    }

    const ALL: [Self; 3] = [Self::Code, Self::General, Self::Personal];

    fn slug(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::General => "general",
            Self::Personal => "personal",
        }
    }
}

fn due_label(d: Option<chrono::NaiveDate>) -> String {
    d.map_or_else(|| "—".to_string(), |d| d.to_string())
}

/// The masthead's status dot — same buckets as [`status_variant`],
/// as a color instead of a pill.
fn status_dot(status: &str) -> &'static str {
    match status {
        "done" | "completed" | "active" => "bg-emerald-500",
        "on_hold" | "on-hold" | "paused" => "bg-amber-500",
        "cancelled" | "canceled" | "archived" => "bg-red-500",
        _ => "bg-muted-foreground",
    }
}

fn status_variant(status: &str) -> StatusBadgeVariant {
    match status {
        "done" | "completed" | "active" => StatusBadgeVariant::Success,
        "on_hold" | "on-hold" | "paused" => StatusBadgeVariant::Warning,
        "cancelled" | "canceled" | "archived" => StatusBadgeVariant::Danger,
        _ => StatusBadgeVariant::Neutral,
    }
}
