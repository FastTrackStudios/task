//! `/projects/:id` — project overview.
//!
//! Resolves one project through the shared optimistic project store
//! ([`crate::stores::use_project`] — instant from cache after a
//! `/projects` visit, else a per-org probe), filters the shared task
//! store's rows to this project (by `project_id` or `[[wikilink]]`),
//! and renders a header with a live progress bar plus the project's
//! tasks through [`task_ui::TasksApp`] (fully editable, optimistic
//! write-through with rollback + tray notification on failure).
//! Threads/messages ride their own stores — posting reconciles the
//! created entity in, no refetch counters anywhere.

use std::collections::BTreeSet;

use agent_proto::session::{Session as AgentSession, SessionStatus as AgentStatus};
use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use architect_ui::prelude::*;
use project_proto::ProjectInfo;
use task_proto::TaskInfo as DbTask;
use task_ui::{TaskInfo as UiTask, TaskMutation, TasksApp};
use timer_proto::WorkSession;
use uuid::Uuid;

use crate::orgs::OrgMeta;
use crate::routes::Route;
use crate::shell::mobile::{BottomSheet, MobileActionBar};
use crate::stores;
use crate::task_sort::{belongs, is_active_task};
use threads::ui::ConversationsPanel;

#[component]
pub fn ProjectDetailView(id: String) -> Element {
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let mut selected_thread = use_signal(|| Option::<Uuid>::None);
    // Mobile add-task sheet — TasksApp hides its inline quick-add on
    // phones; the sticky action bar opens this instead.
    let mut add_open = use_signal(|| false);

    // The project itself: cache-first from the shared store (instant
    // after a /projects visit), else a per-org probe. Mutations (type
    // change, cover image) reconcile straight into this value — no
    // page refresh counter.
    let project_res = stores::use_project(id.clone());
    let project_store = stores::use_project_store();
    let project_muts = stores::use_project_mutations();

    // `(owning slug, project id)` once resolved — the key every
    // dependent fetch hangs off. Mirrored into a reactive memo so the
    // resources/hooks below re-run when it lands or changes.
    let pkey: Option<(String, Uuid)> = project_res.value().map(|r| (r.slug.clone(), r.project.id));
    let pkey_for_memo = pkey.clone();
    let pkey_memo = use_memo(use_reactive!(|(pkey_for_memo,)| pkey_for_memo));

    // The shared task store: the selected orgs' tasks (slug-tagged);
    // `belongs()` filters to this project at render time.
    let tasks_res = stores::use_task_list();
    let _task_store = stores::use_task_store();
    let task_muts = stores::use_task_mutations();

    // Conversations: threads anchored to this project + the selected
    // thread's messages, each its own store (posting reconciles the
    // created entity in).
    let threads_res = stores::use_project_threads(pkey.clone());
    let thread_muts = stores::use_thread_mutations();
    let msg_key: Option<(String, Uuid)> = pkey
        .as_ref()
        .and_then(|(slug, _)| selected_thread().map(|tid| (slug.clone(), tid)));
    let messages_res = stores::use_thread_messages(msg_key);

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
    // below (native parks: the chrome is web-only today).
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
    // join — one extra call, resolved in [`build_budget`]).
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

    // Live task fold: handled by the store layer — the app-root task
    // store subscription (see `stores!`'s `stream:` line) folds every
    // `TaskEvent` from every selected org into the shared store, and
    // `belongs()` filters at render time, so an Upserted task that
    // newly matches the project joins the board and one that stops
    // matching drops out, without this page wiring anything.

    let body = match (project_res.value(), project_res.error()) {
        (Some(op), _) => {
            let p = op.project.clone();
            let forge_slug = op.slug.clone();
            let threads_list: Vec<threads::Thread> = threads_res
                .value()
                .map(|rows| rows.iter().map(|(_, t)| t.clone()).collect())
                .unwrap_or_default();
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

            let msgs: Vec<threads::Message> = messages_res
                .value()
                .map(|rows| rows.iter().map(|(_, m)| m.clone()).collect())
                .unwrap_or_default();
            let sel = *selected_thread.read();
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
            let bd: Option<BudgetData> = budget.read_unchecked().as_ref().cloned().flatten();
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
            let over_budget = p.estimated_seconds > 0
                && bd
                    .as_ref()
                    .is_some_and(|b| b.logged_seconds > p.estimated_seconds);
            // Single-user stand-in identity (matches the timer chrome)
            // so "you" labels your own open session.
            let you: Option<Uuid> = org_list
                .read()
                .iter()
                .find(|o| o.slug == forge_slug)
                .and_then(|o| o.id)
                .map(crate::chrome::owner_id);
            // Implied money budget: estimate × project default rate,
            // when both are set.
            let implied_budget: Option<String> =
                (p.estimated_seconds > 0 && p.default_rate_cents > 0).then(|| {
                    let minor =
                        (p.estimated_seconds as f64 / 3600.0 * p.default_rate_cents as f64) as i64;
                    money_label(minor, &p.currency)
                });

            rsx! {
                Link { to: Route::ProjectsRoute {}, class: "text-xs text-muted-foreground hover:text-foreground",
                    "‹ Projects"
                }

                // ── Cover banner — the `image:` frontmatter, same source
                // the /projects cards render. Set/edited from the Details
                // sidebar below.
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

                // ── Header ──────────────────────────────────────────────
                div { class: "flex flex-col gap-3 rounded-2xl border border-border bg-card/50 p-5",
                    div { class: "flex flex-wrap items-start justify-between gap-3",
                        div { class: "flex min-w-0 flex-col gap-1",
                            div { class: "flex min-w-0 items-center gap-2",
                                if !p.color.is_empty() {
                                    span { class: "h-3 w-3 shrink-0 rounded-full", style: "background:{p.color}" }
                                }
                                Heading { level: HeadingLevel::H1, class: "min-w-0 break-words", "{p.title}" }
                            }
                            div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground",
                                span { class: "capitalize", "{p.priority} priority" }
                                if !p.lead.is_empty() {
                                    span { "· lead {p.lead}" }
                                }
                                if let Some(d) = p.target_date {
                                    span { "· due {d}" }
                                }
                                if let Some(m) = p.date_modified {
                                    span { "· updated {m.date_naive()}" }
                                }
                            }
                        }
                        div { class: "flex items-center gap-2",
                            span { class: "rounded-md border border-border bg-muted/60 px-2 py-0.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground",
                                "{kind.label()}"
                            }
                            StatusBadge { variant: status_variant(&p.status), label: p.status.clone() }
                        }
                    }
                    div { class: "flex items-center gap-3",
                        div { class: "h-2 flex-1 overflow-hidden rounded-full bg-muted",
                            div { class: "h-full rounded-full bg-primary", style: "width: {pct}%" }
                        }
                        span { class: "shrink-0 text-xs font-medium text-muted-foreground", "{done}/{total} tasks · {pct:.0}%" }
                    }
                    if !p.tags.0.is_empty() {
                        div { class: "flex flex-wrap gap-1.5",
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

                // Who's in this project right now — live presence,
                // filtered from the org-wide channel by project id.
                crate::presence::ProjectPresenceStrip { project_id: p.id }

                // The agent lane for this project: questions waiting
                // on a human, runs in flight, branches ready to
                // review. Same component the fleet view at /runners
                // mounts, scoped here — sharing it is what stops the
                // two drifting into different answers.
                crate::pages::agent_surface::AgentSurfaceView {
                    slug: forge_slug.clone(),
                    project: Some(p.id),
                    heading: true,
                }

                // ── Stat tiles ──────────────────────────────────────────
                div { class: "grid grid-cols-2 gap-3 sm:grid-cols-5",
                    StatTile { label: "Tasks", value: "{total}" }
                    StatTile { label: "Done", value: "{done}" }
                    StatTile { label: "Progress", value: "{pct:.0}%" }
                    StatTile { label: "Due", value: due_label(p.target_date) }
                    StatTile { label: "Budget", value: budget_value, warn: over_budget }
                }

                // ── Body grid: main + sidebar ───────────────────────────
                div { class: "grid grid-cols-1 gap-6 lg:grid-cols-3",
                    div { class: "flex flex-col gap-6 lg:col-span-2",
                        if !p.details.trim().is_empty() {
                            div { class: "rounded-xl border border-border bg-card/40 p-4",
                                Text { class: "whitespace-pre-line text-sm leading-relaxed", "{p.details}" }
                            }
                        }
                        // Code projects lead with issues & PRs.
                        if kind == ProjectKind::Code {
                            div { class: "flex flex-col gap-2",
                                Heading { level: HeadingLevel::H2, "Issues & Pull requests" }
                                if connected_repos.is_empty() {
                                    div { class: "rounded-xl border border-dashed border-border px-4 py-6 text-center",
                                        Text { variant: TextVariant::Muted, class: "text-sm",
                                            "Connect a repo (bind it to this project) to track issues & PRs here."
                                        }
                                    }
                                } else {
                                    for rid in connected_repos.iter() {
                                        crate::forge_views::ForgePanel {
                                            key: "{rid.owner}/{rid.repo}",
                                            slug: forge_slug.clone(),
                                            repo_id: rid.clone(),
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex flex-col gap-2",
                            Heading { level: HeadingLevel::H2, "Tasks" }
                            if !active_tasks.is_empty() {
                                div { class: "flex flex-col gap-1 rounded-xl border border-border bg-card/40 p-3",
                                    span { class: "text-[11px] uppercase tracking-wide text-muted-foreground",
                                        "Active"
                                    }
                                    for t in active_tasks.iter() {
                                        ActiveTaskRow { key: "{t.id}", task: t.clone() }
                                    }
                                }
                            }
                    // Always rendered — a fresh project's first task is
                    // one quick-add away, and creates here file under
                    // THIS project (scoped mutations).
                    TasksApp {
                        // No picker on a project board — creates here
                        // are scoped to THIS project already.
                        tasks: mine,
                        on_event: {
                            let create_slug = forge_slug.clone();
                            let scoped = task_muts.scoped_to(p.id);
                            move |mu: TaskMutation| {
                                scoped.apply(&create_slug, mu);
                            }
                        },
                    }
                }
                ActiveNowSection { snapshot: active_snapshot, you }
                ConversationsPanel {
                    threads: threads_list,
                    messages: msgs,
                    selected: sel,
                    on_select: move |id: Uuid| selected_thread.set(Some(id)),
                    on_new_thread: {
                        let slug = forge_slug.clone();
                        let pid = p.id;
                        move |title: String| {
                            let org_id = org_list
                                .read()
                                .iter()
                                .find(|o| o.slug == slug)
                                .and_then(|o| o.id)
                                .unwrap_or_else(Uuid::nil);
                            // Optimistic: the thread appears instantly and
                            // reconciles to the created entity (rollback +
                            // tray notification on failure).
                            thread_muts.create_thread(
                                slug.clone(),
                                threads::CreateThreadRequest {
                                    org_id,
                                    entity_type: "project".into(),
                                    entity_id: pid,
                                    title,
                                    kind: String::new(),
                                    created_by: crate::chrome::owner_id(org_id),
                                    source_kind: "native".into(),
                                    source_ref: None,
                                    source_url: None,
                                },
                            );
                        }
                    },
                    on_post: {
                        let slug = forge_slug.clone();
                        move |body: String| {
                            let Some(tid) = *selected_thread.read() else {
                                return;
                            };
                            let org_id = org_list
                                .read()
                                .iter()
                                .find(|o| o.slug == slug)
                                .and_then(|o| o.id)
                                .unwrap_or_else(Uuid::nil);
                            // Optimistic: the message appears instantly and
                            // reconciles to the posted entity.
                            thread_muts.post_message(
                                slug.clone(),
                                threads::PostMessageRequest {
                                    thread_id: tid,
                                    org_id,
                                    author_id: Some(crate::chrome::owner_id(org_id)),
                                    author_label: "me".into(),
                                    body,
                                    reply_to: None,
                                    source_kind: "native".into(),
                                    external_id: None,
                                    original_text: None,
                                    source_url: None,
                                    posted_at: None,
                                },
                            );
                        }
                    },
                }
                // General projects show the repo (when connected) after
                // their tasks/conversations. Code shows it up top; personal
                // hides it entirely.
                if kind == ProjectKind::General && !connected_repos.is_empty() {
                    div { class: "flex flex-col gap-2",
                        Heading { level: HeadingLevel::H2, "Repository" }
                        for rid in connected_repos.iter() {
                            crate::forge_views::ForgePanel {
                                key: "{rid.owner}/{rid.repo}",
                                slug: forge_slug.clone(),
                                repo_id: rid.clone(),
                            }
                        }
                    }
                }
                    }
                    // ── Sidebar: details ────────────────────────────────
                    div { class: "flex flex-col gap-4",
                        div { class: "flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4",
                            Heading { level: HeadingLevel::H3, "Details" }
                            // Editable project type / template.
                            div { class: "flex items-center justify-between gap-3 text-sm",
                                span { class: "shrink-0 text-muted-foreground", "Type" }
                                div { class: "flex gap-1",
                                    for k in ProjectKind::ALL {
                                        {
                                            let np_base = p.clone();
                                            let type_slug = forge_slug.clone();
                                            let is_current = k == kind;
                                            rsx! {
                                                Button {
                                                    key: "{k.slug()}",
                                                    variant: if is_current { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                                                    size: ButtonSize::Small,
                                                    on_click: move |_| {
                                                        // Optimistic: the badge flips instantly;
                                                        // a failed write rolls back + notifies.
                                                        let mut np = np_base.clone();
                                                        np.project_type = k.slug().to_string();
                                                        project_muts.update(type_slug.clone(), np);
                                                    },
                                                    "{k.label()}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            DetailRow { label: "Status".to_string(), value: p.status.clone() }
                            DetailRow { label: "Priority".to_string(), value: p.priority.clone() }
                            if !p.lead.is_empty() {
                                DetailRow { label: "Lead".to_string(), value: p.lead.clone() }
                            }
                            DetailRow { label: "Due".to_string(), value: due_label(p.target_date) }
                            if p.estimated_seconds > 0 {
                                DetailRow { label: "Estimate".to_string(), value: hours_label(p.estimated_seconds) }
                            }
                            if let Some(c) = p.date_created {
                                DetailRow { label: "Created".to_string(), value: c.date_naive().to_string() }
                            }
                            if let Some(m) = p.date_modified {
                                DetailRow { label: "Updated".to_string(), value: m.date_naive().to_string() }
                            }
                            CoverImageEditor {
                                project: p.clone(),
                                slug: forge_slug.clone(),
                            }
                        }
                        // ── Budget card ─────────────────────────────
                        div { class: "flex flex-col gap-3 rounded-xl border border-border bg-card/40 p-4",
                            Heading { level: HeadingLevel::H3, "Budget" }
                            match &bd {
                                Some(b) => rsx! {
                                    if p.estimated_seconds > 0 {
                                        DetailRow {
                                            label: "Time".to_string(),
                                            value: format!(
                                                "{} of {}",
                                                hours_label(b.logged_seconds),
                                                hours_label(p.estimated_seconds),
                                            ),
                                        }
                                        if b.logged_seconds > p.estimated_seconds {
                                            div { class: "rounded-md border border-amber-500/40 bg-amber-500/10 px-2 py-1 text-xs text-amber-600 dark:text-amber-400",
                                                "Over the time budget by {hours_label(b.logged_seconds - p.estimated_seconds)}"
                                            }
                                        }
                                    } else {
                                        DetailRow { label: "Logged".to_string(), value: hours_label(b.logged_seconds) }
                                    }
                                    if let Some(budget) = implied_budget.clone() {
                                        DetailRow { label: "Budget (est × rate)".to_string(), value: budget }
                                    }
                                    if let Some((minor, cur)) = &b.unbilled {
                                        DetailRow { label: "Unbilled".to_string(), value: money_label(*minor, cur) }
                                    }
                                    if b.has_invoices {
                                        DetailRow {
                                            label: "Invoiced".to_string(),
                                            value: money_label(b.invoiced_minor, &b.invoice_currency),
                                        }
                                        DetailRow {
                                            label: "Paid".to_string(),
                                            value: money_label(b.paid_minor, &b.invoice_currency),
                                        }
                                    }
                                    if p.estimated_seconds == 0 && b.unbilled.is_none() && !b.has_invoices && b.logged_seconds == 0 {
                                        Text { variant: TextVariant::Muted, class: "text-sm",
                                            "No estimate, logged time, or billing yet."
                                        }
                                    }
                                },
                                None => rsx! {
                                    Text { variant: TextVariant::Muted, class: "text-sm", "Loading budget…" }
                                },
                            }
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
        div { class: "mx-auto w-full max-w-6xl flex flex-col gap-6 p-4 pb-14 sm:p-6 md:pb-6 lg:p-10", {body} }
    }
}

/// A big-number overview tile (Tasks / Done / Progress / Due /
/// Budget). `warn` renders the value amber — the over-budget signal.
#[component]
fn StatTile(label: String, value: String, #[props(default)] warn: bool) -> Element {
    let value_class = if warn {
        "text-2xl font-semibold tabular-nums text-amber-500"
    } else {
        "text-2xl font-semibold tabular-nums"
    };
    rsx! {
        div { class: "flex flex-col gap-0.5 rounded-xl border border-border bg-card/40 p-3",
            span { class: value_class, "{value}" }
            span { class: "text-[11px] uppercase tracking-wide text-muted-foreground", "{label}" }
        }
    }
}

// ── Budget ──────────────────────────────────────────────────────────

/// Aggregated spend for one project, assembled client-side from the
/// timer + invoicing feeds.
#[derive(Clone, PartialEq, Default)]
struct BudgetData {
    /// Σ session durations; an open timer counts up to "now".
    logged_seconds: i64,
    /// `(amount_minor, currency)` from the project's
    /// [`finance_proto::UninvoicedGroup`], when one exists.
    unbilled: Option<(i64, String)>,
    /// Σ `total_minor` of the invoices this project's sessions were
    /// billed on.
    invoiced_minor: i64,
    /// Σ `amount_paid_minor` of the same invoices.
    paid_minor: i64,
    /// Currency of those invoices (one project = one currency).
    invoice_currency: String,
    /// Whether the `session.invoice_id → invoice` join resolved any
    /// invoice at all — gates the Invoiced / Paid rows.
    has_invoices: bool,
}

/// Fold the project's sessions + the org's uninvoiced groups +
/// invoice list into one [`BudgetData`]. Pure (injected `now`) so the
/// duration math is testable.
///
/// The invoiced / paid join: invoices are generated per-project
/// (`GenerateInvoice` takes a project), so summing the invoices
/// referenced by this project's `session.invoice_id`s is exact and
/// costs a single `list_invoices` call — cheap enough to resolve
/// client-side rather than descope.
fn build_budget(
    pid: Uuid,
    sessions: &[WorkSession],
    uninvoiced: &[finance_proto::UninvoicedGroup],
    invoices: &[finance_proto::Invoice],
    now: DateTime<Utc>,
) -> BudgetData {
    let logged_seconds = sessions
        .iter()
        .map(|s| {
            (s.end_time.unwrap_or(now) - s.start_time)
                .num_seconds()
                .max(0)
        })
        .sum();
    let unbilled = uninvoiced
        .iter()
        .find(|g| g.project_id == Some(pid))
        .map(|g| (g.amount_minor, g.currency.clone()));
    let billed_on: BTreeSet<Uuid> = sessions.iter().filter_map(|s| s.invoice_id).collect();
    let mut out = BudgetData {
        logged_seconds,
        unbilled,
        ..Default::default()
    };
    for inv in invoices.iter().filter(|i| billed_on.contains(&i.id)) {
        out.has_invoices = true;
        out.invoiced_minor += inv.total_minor;
        out.paid_minor += inv.amount_paid_minor;
        if out.invoice_currency.is_empty() {
            out.invoice_currency = inv.currency.clone();
        }
    }
    out
}

/// The Budget stat-tile text: "logged / estimated" in hours. The
/// caller renders "—" when there's no estimate at all.
fn budget_tile_value(logged_seconds: i64, estimated_seconds: i64) -> String {
    format!(
        "{} / {}",
        hours_label(logged_seconds),
        hours_label(estimated_seconds)
    )
}

/// `amount_minor` + ISO currency → display string. Minor units are
/// hundredths (cents); an empty currency falls back to `$` like the
/// finances / invoices pages.
fn money_label(amount_minor: i64, currency: &str) -> String {
    let amount = amount_minor as f64 / 100.0;
    if currency.is_empty() {
        format!("${amount:.2}")
    } else {
        format!("{amount:.2} {currency}")
    }
}

// ── Active now ──────────────────────────────────────────────────────

/// Open timers + in-flight agent sessions. `snapshot == None` while
/// the first fetch is in flight (quiet placeholder, no blanking);
/// empty lists render the nobody-active state.
#[component]
fn ActiveNowSection(
    snapshot: Option<(Vec<WorkSession>, Vec<AgentSession>)>,
    you: Option<Uuid>,
) -> Element {
    rsx! {
        div { class: "flex flex-col gap-2",
            Heading { level: HeadingLevel::H2, "Active now" }
            match &snapshot {
                None => rsx! {
                    Text { variant: TextVariant::Muted, class: "text-sm", "Checking…" }
                },
                Some((timers, agents)) if timers.is_empty() && agents.is_empty() => rsx! {
                    div { class: "rounded-xl border border-dashed border-border px-4 py-6 text-center",
                        Text { variant: TextVariant::Muted, class: "text-sm",
                            "Nobody's working on this project right now."
                        }
                    }
                },
                Some((timers, agents)) => rsx! {
                    div { class: "flex flex-col divide-y divide-border/50 rounded-xl border border-border/60 bg-card/40",
                        for s in timers.iter() {
                            TimerRow { key: "{s.id}", session: s.clone(), you }
                        }
                        for s in agents.iter() {
                            AgentRow { key: "{s.id}", session: s.clone() }
                        }
                    }
                },
            }
        }
    }
}

/// One open timer session: who, what, since when, on which task.
#[component]
fn TimerRow(session: WorkSession, you: Option<Uuid>) -> Element {
    let who = if Some(session.user_id) == you {
        "you".to_string()
    } else {
        // No member directory yet — fall back to a short stable id.
        session.user_id.to_string()[..8].to_string()
    };
    let what = if session.description.trim().is_empty() {
        "(no description)".to_string()
    } else {
        session.description.clone()
    };
    let since = ago_label(Utc::now(), session.start_time);
    rsx! {
        div { class: "flex items-center justify-between gap-3 px-3 py-2.5",
            div { class: "flex min-w-0 flex-col gap-0.5",
                div { class: "flex items-center gap-2 text-sm",
                    span { class: "font-medium", "{who}" }
                    span { class: "truncate text-muted-foreground", "{what}" }
                }
                div { class: "flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground",
                    span { "started {since}" }
                    if !session.task_note_path.is_empty() {
                        span { "·" }
                        span { class: "truncate font-mono", "{session.task_note_path}" }
                    }
                }
            }
            StatusBadge { variant: StatusBadgeVariant::Success, label: "timer".to_string() }
        }
    }
}

/// One in-flight agent session: nickname / title, status, what it's
/// chewing on, last activity.
#[component]
fn AgentRow(session: AgentSession) -> Element {
    let name = if !session.subagent_nickname.is_empty() {
        session.subagent_nickname.clone()
    } else if !session.title.trim().is_empty() {
        session.title.clone()
    } else {
        "(untitled agent)".to_string()
    };
    let turn = session
        .pending
        .as_ref()
        .map(|pt| turn_summary(&pt.user_message));
    let last = session.last_message_at.map(|t| ago_label(Utc::now(), t));
    rsx! {
        div { class: "flex items-center justify-between gap-3 px-3 py-2.5",
            div { class: "flex min-w-0 flex-col gap-0.5",
                span { class: "truncate text-sm font-medium", "{name}" }
                div { class: "flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground",
                    if let Some(t) = turn {
                        span { class: "truncate", "{t}" }
                    }
                    if let Some(l) = last {
                        span { "·" }
                        span { "{l}" }
                    }
                }
            }
            StatusBadge {
                variant: agent_status_variant(session.status),
                label: agent_status_label(session.status).to_string(),
            }
        }
    }
}

fn agent_status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "Idle",
        AgentStatus::Running => "Running",
        AgentStatus::AwaitingUser => "Awaiting user",
        AgentStatus::Cancelled => "Cancelled",
        AgentStatus::Errored => "Errored",
    }
}

fn agent_status_variant(status: AgentStatus) -> StatusBadgeVariant {
    match status {
        AgentStatus::Running => StatusBadgeVariant::Success,
        AgentStatus::AwaitingUser => StatusBadgeVariant::Warning,
        AgentStatus::Errored => StatusBadgeVariant::Danger,
        AgentStatus::Idle | AgentStatus::Cancelled => StatusBadgeVariant::Neutral,
    }
}

/// First line of the pending user message, clipped — the "what is it
/// doing" summary in the agent row.
fn turn_summary(msg: &str) -> String {
    let line = msg.lines().next().unwrap_or_default().trim();
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        out.push('…');
    }
    out
}

/// Compact relative time ("just now" / "5m ago" / "3h ago" / "2d
/// ago"). Pure (injected `now`) so it's testable.
fn ago_label(now: DateTime<Utc>, t: DateTime<Utc>) -> String {
    let secs = (now - t).num_seconds().max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// 30-second poll cadence for the active-now / budget refresh. Native
/// parks (no poll) — same pattern as the chrome's second tick.
#[cfg(target_arch = "wasm32")]
async fn sleep_30s() {
    gloo_timers::future::TimeoutFuture::new(30_000).await;
}

#[cfg(not(target_arch = "wasm32"))]
async fn sleep_30s() {
    futures_util::future::pending::<()>().await;
}

// ── Active tasks ────────────────────────────────────────────────────

/// Comma-joined holder labels from `workflow.assignees`.
fn assignee_labels(t: &DbTask) -> String {
    use task_proto::workflows_proto::AgentRef;
    let Some(w) = &t.workflow else {
        return String::new();
    };
    w.assignees
        .0
        .iter()
        .map(|a| match a {
            AgentRef::Human {
                user_id,
                display_name,
            } => display_name.clone().unwrap_or_else(|| user_id.clone()),
            AgentRef::Agent { name, .. } => format!("agent:{name}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// One row in the "Active" slice: title (links to the task detail),
/// holders, status badge.
#[component]
fn ActiveTaskRow(task: ReadSignal<DbTask>) -> Element {
    let task = task.read();
    let holders = assignee_labels(&task);
    rsx! {
        div { class: "flex items-center justify-between gap-3 py-1 text-sm",
            Link {
                to: Route::TaskDetailRoute { id: task.id },
                class: "min-w-0 truncate font-medium hover:underline",
                "{task.title}"
            }
            div { class: "flex shrink-0 items-center gap-2",
                if !holders.is_empty() {
                    span { class: "text-xs text-muted-foreground", "{holders}" }
                }
                StatusBadge { variant: status_variant(&task.status), label: task.status.clone() }
            }
        }
    }
}

/// Sidebar editor for the project's cover image — writes through the
/// `image:` frontmatter via the optimistic project store, the same
/// idiom as the Type buttons above it (instant preview, rollback +
/// tray notification on failure).
///
/// Smallest honest slice: paste an image URL (or any path the browser
/// can resolve) and save; the /projects cards and the detail banner
/// render it. Uploading bytes through the `AttachmentService` is
/// deliberately *not* wired yet: its download URLs are short-lived
/// signed links, so persisting one into `image:` would go stale — that
/// needs a stable blob URL (or render-time hash → URL resolution)
/// first. The gap is documented on the issue.
#[component]
fn CoverImageEditor(project: ProjectInfo, slug: String) -> Element {
    let muts = stores::use_project_mutations();
    let mut draft = use_signal(|| project.image.clone());
    // Re-sync the draft when a reconciled write lands a new stored value.
    let stored = project.image.clone();
    use_effect(use_reactive!(|(stored,)| draft.set(stored)));

    let preview = draft.read().trim().to_string();
    let dirty = preview != project.image.trim();
    let busy = muts.is_pending();
    let error = muts.error();

    rsx! {
        div { class: "flex flex-col gap-1.5 text-sm",
            span { class: "text-muted-foreground", "Cover image" }
            if !preview.is_empty() {
                div { class: "aspect-video w-full overflow-hidden rounded-md border border-border bg-muted",
                    img {
                        src: "{preview}",
                        alt: "Cover preview",
                        loading: "lazy",
                        class: "h-full w-full object-cover",
                    }
                }
            }
            div { class: "flex items-center gap-2",
                Input {
                    value: draft,
                    placeholder: "https://… image URL",
                    on_change: move |_| {},
                }
                if dirty {
                    Button {
                        variant: ButtonVariant::Secondary,
                        size: ButtonSize::Small,
                        disabled: busy,
                        on_click: {
                            let np_base = project.clone();
                            let save_slug = slug.clone();
                            move |_| {
                                let mut np = np_base.clone();
                                np.image = draft.peek().trim().to_string();
                                muts.update(save_slug.clone(), np);
                            }
                        },
                        if busy { "Saving…" } else { "Save" }
                    }
                }
            }
            if let Some(e) = error.as_ref() {
                span { class: "text-xs text-destructive", "Couldn't save the cover: {e}" }
            }
        }
    }
}

/// A label/value row in the sidebar Details card.
#[component]
fn DetailRow(label: String, value: String) -> Element {
    rsx! {
        div { class: "flex items-baseline justify-between gap-3 text-sm",
            span { class: "shrink-0 text-muted-foreground", "{label}" }
            span { class: "min-w-0 break-words text-right font-medium", "{value}" }
        }
    }
}

fn due_label(d: Option<chrono::NaiveDate>) -> String {
    d.map_or_else(|| "—".to_string(), |d| d.to_string())
}

fn hours_label(secs: i64) -> String {
    format!("{:.1}h", secs as f64 / 3600.0)
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

fn status_variant(status: &str) -> StatusBadgeVariant {
    match status {
        "done" | "completed" | "active" => StatusBadgeVariant::Success,
        "on_hold" | "on-hold" | "paused" => StatusBadgeVariant::Warning,
        "cancelled" | "canceled" | "archived" => StatusBadgeVariant::Danger,
        _ => StatusBadgeVariant::Neutral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_label_formats_minor_units_with_currency() {
        assert_eq!(money_label(123_456, "USD"), "1234.56 USD");
        assert_eq!(money_label(0, "EUR"), "0.00 EUR");
        // Empty currency falls back to `$` like the finances page.
        assert_eq!(money_label(995, ""), "$9.95");
        assert_eq!(money_label(-2_500, "USD"), "-25.00 USD");
    }

    #[test]
    fn budget_tile_shows_logged_vs_estimated_hours() {
        assert_eq!(budget_tile_value(5_400, 36_000), "1.5h / 10.0h");
        assert_eq!(budget_tile_value(0, 3_600), "0.0h / 1.0h");
    }

    #[test]
    fn hours_label_rounds_to_tenths() {
        assert_eq!(hours_label(3_600), "1.0h");
        assert_eq!(hours_label(5_430), "1.5h");
        assert_eq!(hours_label(0), "0.0h");
    }

    #[test]
    fn ago_label_buckets() {
        let now = Utc::now();
        let s = |secs: i64| now - chrono::Duration::seconds(secs);
        assert_eq!(ago_label(now, s(10)), "just now");
        assert_eq!(ago_label(now, s(90)), "1m ago");
        assert_eq!(ago_label(now, s(2 * 3_600 + 60)), "2h ago");
        assert_eq!(ago_label(now, s(3 * 86_400)), "3d ago");
        // Clock skew (event in the future) clamps to "just now".
        assert_eq!(
            ago_label(now, now + chrono::Duration::seconds(30)),
            "just now"
        );
    }

    #[test]
    fn turn_summary_first_line_clipped() {
        assert_eq!(
            turn_summary("fix the build\nand then more"),
            "fix the build"
        );
        let long = "x".repeat(120);
        let clipped = turn_summary(&long);
        assert_eq!(clipped.chars().count(), 81); // 80 + ellipsis
        assert!(clipped.ends_with('…'));
        assert_eq!(turn_summary(""), "");
    }
}
