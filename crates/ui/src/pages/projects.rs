//! `/projects` — the workspace's project command center.
//!
//! Reads the shared optimistic project store ([`crate::stores`]) —
//! slug-tagged rows fetched across the selected orgs via the
//! architect-generated `ProjectServiceClient` — then renders a live
//! stats band over a grid of rich project cards. Visiting this page
//! hydrates the store, so `/projects/:id` resolves from cache.
//!
//! Each card carries a data-driven accent (the project's own `color`,
//! or a stable pick from the chart palette), a real progress bar, and
//! priority / lead / due metadata. Subprojects nest as a compact,
//! status-dotted list inside their parent card.
//!
//! Loading paints skeleton cards; the empty and error states are
//! first-class. Everything is theme-token only (no hex in styling) so
//! it tracks light/dark automatically — dark is the default.

use crate::format::status_variant;
use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{
    CalendarDays, Flag, FolderKanban, Layers, LayoutGrid, LayoutList, Plus, User,
};
use architect_ui::prelude::*;
use project_proto::ProjectInfo;

use crate::routes::Route;
use crate::shell::mobile::{BottomSheet, MobileActionBar};
use crate::task_sort::{priority_rank, status_rank};

/// Page-scoped keyframes for the staggered card entrance. Injected once
/// per render as a plain `<style>` (idempotent — identical content).
const PAGE_CSS: &str = "@keyframes ftsFadeUp{from{opacity:0;transform:translateY(14px)}to{opacity:1;transform:translateY(0)}}";

/// How the project list is laid out. Persisted for the page's lifetime.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ViewMode {
    Cards,
    #[default]
    List,
}

/// List, always. Cards are the browsing view; the list is the working
/// one — it fits three times as many projects on screen and every row
/// still carries its image, progress and status.
fn initial_view_mode() -> ViewMode {
    ViewMode::List
}

/// The loaded page's shape, derived once per project-data change
/// (filter → sort → group → stats all walk the whole list).
#[derive(Clone, PartialEq)]
struct ProjectsData {
    /// Each top-level project with its (non-archived) subprojects,
    /// active-first / priority / title order.
    groups: Vec<(ProjectInfo, Vec<ProjectInfo>)>,
    total: usize,
    active: usize,
    on_hold: usize,
    stale: usize,
    /// Top-level projects the Relevant filter is holding back. Shown
    /// as a count, because a filter that hides things silently is
    /// indistinguishable from data that never loaded.
    hidden: usize,
    avg: Option<i32>,
}

#[component]
pub fn ProjectsView() -> Element {
    // The shared optimistic project store: one AtomResult for the list
    // (stale-while-revalidate across org switches), and the cache that
    // makes `/projects/:id` instant after this visit.
    let projects = crate::stores::use_project_list();
    let store = crate::stores::use_project_store();
    let muts = crate::stores::use_project_mutations();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let view_mode = use_signal(initial_view_mode);
    // Relevant defaults ON: of a hundred projects, seven are current.
    // Opening on the full list would bury them.
    let relevant = use_signal(|| true);
    // Mobile create sheet — the desktop header (and its quick-add) is
    // hidden below `md`, so the sticky action bar owns creation there.
    let mut new_open = use_signal(|| false);

    let derived = use_memo(move || {
        let rows: Vec<ProjectInfo> = store.list().into_iter().map(|r| r.project).collect();
        shape_projects(&rows, relevant())
    });

    // The list is a multi-org fan-out, so "which org owns this project"
    // is per-row, not per-page. Only the cover tile needs it (a media
    // grant is signed per org, and one signed for the wrong org is
    // simply rejected), so it travels as context rather than as a prop
    // threaded through every card, row and group in between.
    let cover_orgs = use_memo(move || {
        store
            .list()
            .into_iter()
            .map(|r| (r.project.id, r.slug))
            .collect::<CoverOrgs>()
    });
    use_context_provider(move || cover_orgs);

    let org_choices: Vec<(String, String)> = org_list
        .read()
        .iter()
        .map(|o| (o.slug.clone(), o.name.clone()))
        .collect();
    let quick_add = rsx! {
        ProjectQuickAdd {
            orgs: org_choices,
            default_slug: crate::orgs::create_target(&selection.read(), &org_list.read()),
            on_create: move |(slug, title): (String, String)| {
                muts.create(slug, crate::stores::draft_project(title));
            },
        }
    };

    let view = match (projects.value(), projects.error()) {
        (Some(_), _) => render_loaded(&derived.read(), view_mode, relevant, quick_add),
        (None, Some(e)) => rsx! {
            page_header { count_line: None, hidden: 0 }
            crate::states::ErrorState {
                title: "Couldn't reach the project service",
                message: e.clone(),
                on_retry: move |()| store.reload(),
            }
        },
        (None, None) => render_loading(),
    };

    rsx! {
        style { dangerous_inner_html: PAGE_CSS }
        div { class: "relative isolate",
            // Atmospheric wash behind the header — token-only via color-mix.
            div {
                class: "pointer-events-none absolute inset-x-0 top-0 -z-10 h-72",
                style: "background: radial-gradient(60% 120% at 50% -10%, color-mix(in oklch, var(--primary) 10%, transparent), transparent 70%);",
            }
            div { class: "mx-auto w-full max-w-6xl flex flex-col gap-8 p-4 pb-14 sm:p-6 md:pb-6 lg:p-10",
                {view}
            }
        }
        // ── Mobile: sticky create above the tab bar ─────────────────
        MobileActionBar {
            button {
                r#type: "button",
                class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg bg-primary px-3 py-2 text-sm font-medium text-primary-foreground active:bg-primary/85",
                onclick: move |_| new_open.set(true),
                Plus { size: 16 }
                "New project"
            }
        }
        BottomSheet {
            open: new_open(),
            title: "New project".to_string(),
            on_close: move |()| new_open.set(false),
            div { class: "pb-2",
                ProjectQuickAdd {
                    orgs: org_list
                        .read()
                        .iter()
                        .map(|o| (o.slug.clone(), o.name.clone()))
                        .collect::<Vec<_>>(),
                    default_slug: crate::orgs::create_target(&selection.read(), &org_list.read()),
                    on_create: move |(slug, title): (String, String)| {
                        muts.create(slug, crate::stores::draft_project(title));
                        new_open.set(false);
                    },
                }
            }
        }
    }
}

// ── states ──────────────────────────────────────────────────────────

fn render_loading() -> Element {
    rsx! {
        header { class: "flex flex-col gap-3",
            div { class: "h-3 w-28 animate-pulse rounded-full bg-muted" }
            div { class: "h-9 w-56 animate-pulse rounded-lg bg-muted" }
        }
        div { class: "grid grid-cols-2 gap-px overflow-hidden rounded-2xl border border-border/70 bg-border/70 sm:grid-cols-4",
            for i in 0..4 {
                div { key: "{i}", class: "flex flex-col gap-2 bg-card p-5",
                    div { class: "h-8 w-14 animate-pulse rounded-md bg-muted" }
                    div { class: "h-3 w-20 animate-pulse rounded-full bg-muted" }
                }
            }
        }
        div { class: "grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3",
            for i in 0..6 {
                div { key: "{i}", class: "flex flex-col gap-4 rounded-xl border border-border/70 bg-card p-6",
                    div { class: "flex items-start justify-between gap-3",
                        div { class: "h-5 w-32 animate-pulse rounded-md bg-muted" }
                        div { class: "h-6 w-16 animate-pulse rounded-full bg-muted" }
                    }
                    div { class: "h-1.5 w-full animate-pulse rounded-full bg-muted" }
                    div { class: "flex gap-2",
                        div { class: "h-3 w-16 animate-pulse rounded-full bg-muted" }
                        div { class: "h-3 w-20 animate-pulse rounded-full bg-muted" }
                    }
                }
            }
        }
    }
}

/// Filter/sort/group the raw rows into [`ProjectsData`]. Pure — called
/// from the page's `use_memo`.
/// Is this project one the user should be looking at?
///
/// `Status::is_current()` — active or deliberately on hold. Stale,
/// done and cancelled are not. An unrecognized custom status counts as
/// current: a project the vocabulary doesn't know about is not
/// something to hide.
fn is_relevant(p: &ProjectInfo) -> bool {
    project_proto::Status::from_str(&p.status).is_none_or(project_proto::Status::is_current)
}

fn shape_projects(rows: &[ProjectInfo], relevant_only: bool) -> ProjectsData {
    // Archived projects stay out of the main grid.
    let live: Vec<&ProjectInfo> = rows.iter().filter(|p| !p.archived).collect();

    let all_top = live.iter().filter(|p| p.parent_id.is_none()).count();
    let mut top: Vec<&ProjectInfo> = live
        .iter()
        .filter(|p| p.parent_id.is_none())
        .filter(|p| !relevant_only || is_relevant(p))
        .copied()
        .collect();
    let hidden = all_top - top.len();
    // Stable, useful order: active first, then by priority, then title.
    top.sort_by(|a, b| {
        status_rank(&a.status)
            .cmp(&status_rank(&b.status))
            .then_with(|| priority_rank(&a.priority).cmp(&priority_rank(&b.priority)))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    let total = live.len();
    let active = live
        .iter()
        .filter(|p| matches_status(&p.status, Bucket::Active))
        .count();
    let on_hold = live
        .iter()
        .filter(|p| matches_status(&p.status, Bucket::Hold))
        .count();
    let stale = live
        .iter()
        .filter(|p| {
            project_proto::Status::from_str(&p.status) == Some(project_proto::Status::Stale)
        })
        .count();
    let tracked: Vec<i16> = live
        .iter()
        .filter(|p| p.progress_percent >= 0)
        .map(|p| p.progress_percent)
        .collect();
    let avg = if tracked.is_empty() {
        None
    } else {
        Some(tracked.iter().map(|v| i32::from(*v)).sum::<i32>() / tracked.len() as i32)
    };

    // Materialize each top-level project with its subprojects once, so
    // both layouts share the same data shaping.
    let groups: Vec<(ProjectInfo, Vec<ProjectInfo>)> = top
        .iter()
        .map(|parent| {
            let kids: Vec<ProjectInfo> = rows
                .iter()
                .filter(|p| !p.archived && p.parent_id == Some(parent.id))
                .cloned()
                .collect();
            ((*parent).clone(), kids)
        })
        .collect();

    ProjectsData {
        groups,
        total,
        active,
        on_hold,
        stale,
        hidden,
        avg,
    }
}

fn render_loaded(
    data: &ProjectsData,
    view_mode: Signal<ViewMode>,
    relevant: Signal<bool>,
    quick_add: Element,
) -> Element {
    if data.total == 0 {
        return rsx! {
            page_header {
                count_line: None,
                view_mode: None,
                relevant: None,
                hidden: 0,
                quick_add: Some(quick_add.clone()),
            }
            div { class: "flex flex-col items-center gap-4 rounded-2xl border border-dashed border-border/70 bg-card/40 px-6 py-16 text-center",
                div { class: "flex size-14 items-center justify-center rounded-2xl bg-muted text-muted-foreground",
                    FolderKanban { size: 26 }
                }
                div { class: "flex flex-col gap-1",
                    Heading { level: HeadingLevel::H3, "No projects yet" }
                    Text {
                        variant: TextVariant::Muted,
                        "Seed a `type: project` page under `vault/Projects/` and it'll appear here."
                    }
                }
            }
        };
    }

    let ProjectsData {
        groups,
        total,
        active,
        on_hold,
        stale,
        hidden,
        avg,
    } = data;
    let count_line = format!("{} shown · {total} total", groups.len());

    rsx! {
        page_header {
            count_line: Some(count_line),
            view_mode: Some(view_mode),
            relevant: Some(relevant),
            hidden: *hidden,
            quick_add: Some(quick_add),
        }

        // ── live stats band — hairline-divided tiles ───────────────
        div { class: "hidden grid-cols-2 gap-px overflow-hidden rounded-2xl border border-border/70 bg-border/70 md:grid md:grid-cols-4",
            Stat { value: "{active}", label: "Active".to_string(), hint: None }
            Stat { value: "{on_hold}", label: "On hold".to_string(), hint: None }
            Stat { value: "{stale}", label: "Stale".to_string(), hint: None }
            Stat {
                value: match avg { Some(a) => format!("{a}%"), None => "—".to_string() },
                label: "Avg progress".to_string(),
                hint: None,
            }
        }

        div { class: "pointer-events-none fixed right-3 top-20 z-40 md:hidden",
            MobileViewToggle { view: view_mode }
        }

        if groups.is_empty() {
            div { class: "flex flex-col items-center gap-3 rounded-2xl border border-dashed border-border/70 bg-card/40 px-6 py-14 text-center",
                Heading { level: HeadingLevel::H3, "Nothing current" }
                Text {
                    variant: TextVariant::Muted,
                    "Every project here is stale, done or cancelled. Turn off Relevant to see them."
                }
            }
        }

        match view_mode() {
            ViewMode::Cards => rsx! {
                // Equal-height cards: the grid stretches each cell to the
                // row's tallest, and the card fills it (`h-full`).
                div { class: "grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3",
                    for (i, (parent, kids)) in groups.iter().enumerate() {
                        ProjectCardView {
                            key: "{parent.id}",
                            p: parent.clone(),
                            subprojects: kids.clone(),
                            index: i,
                        }
                    }
                }
            },
            ViewMode::List => rsx! {
                div { class: "overflow-hidden rounded-xl border border-border/70",
                    for (i, (parent, kids)) in groups.iter().enumerate() {
                        ProjectRow {
                            key: "{parent.id}",
                            p: parent.clone(),
                            sub_count: kids.len(),
                            first: i == 0,
                        }
                    }
                }
            },
        }
    }
}

// ── header ──────────────────────────────────────────────────────────

/// Slim project quick-create: type a name, hit enter, the project
/// exists (optimistically — the store insert lands instantly, the
/// backend scaffolds `Projects/<slug>.md` with active status). When
/// more than one org is hosted, an inline select chooses where it
/// lands (defaulting to the org-switcher's create target). The
/// dashboard reuses it in `compact` form.
#[component]
pub(crate) fn ProjectQuickAdd(
    /// `(slug, display name)` of every org the project could land in.
    orgs: Vec<(String, String)>,
    /// Pre-selected slug (the org-switcher's create target).
    default_slug: String,
    on_create: EventHandler<(String, String)>,
    #[props(default)] compact: bool,
) -> Element {
    let mut value = use_signal(String::new);
    let chosen = use_signal(|| default_slug.clone());
    let orgs_for_submit = orgs.clone();
    let mut submit = move || {
        let title = value.read().trim().to_string();
        if title.is_empty() {
            return;
        }
        let slug = {
            let c = chosen.read().clone();
            if orgs_for_submit.iter().any(|(s, _)| *s == c) {
                c
            } else {
                orgs_for_submit.first().map(|(s, _)| s.clone()).unwrap_or(c)
            }
        };
        on_create.call((slug, title));
        value.set(String::new());
    };
    let (form_cls, placeholder) = if compact {
        (
            "flex w-56 items-center gap-1.5 rounded-lg border border-border/60 bg-transparent px-2 py-1 focus-within:border-border focus-within:bg-card focus-within:ring-2 focus-within:ring-primary/50",
            "New project…",
        )
    } else {
        (
            "flex max-w-md items-center gap-2 rounded-lg border border-border/60 bg-transparent px-3 py-1.5 focus-within:border-border focus-within:bg-card focus-within:ring-2 focus-within:ring-primary/50",
            "New project — name it and go",
        )
    };
    rsx! {
        form {
            class: "{form_cls}",
            onsubmit: move |e: Event<FormData>| {
                e.prevent_default();
                submit();
            },
            span { class: "flex h-5 w-5 items-center justify-center rounded-md bg-primary/15 text-primary",
                FolderKanban { size: 12 }
            }
            input {
                r#type: "text",
                value: "{value}",
                placeholder,
                class: "min-w-0 flex-1 bg-transparent text-sm text-foreground placeholder:text-muted-foreground outline-none",
                oninput: move |e| value.set(e.value()),
            }
            // Which org gets it — only a question when several could.
            if orgs.len() > 1 {
                Select {
                    value: chosen,
                    placeholder: "Org".to_string(),
                    class: "shrink-0".to_string(),
                    SelectContent {
                        for (i, (slug, name)) in orgs.iter().enumerate() {
                            SelectItem { key: "{slug}", value: "{slug}", index: i, "{name}" }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct PageHeaderProps {
    count_line: Option<String>,
    view_mode: Option<Signal<ViewMode>>,
    #[props(default)]
    quick_add: Option<Element>,
    /// The Relevant toggle. `None` on the empty state, where filtering
    /// nothing would only confuse.
    relevant: Option<Signal<bool>>,
    /// How many top-level projects the filter is holding back.
    hidden: usize,
}

#[component]
fn page_header(props: PageHeaderProps) -> Element {
    rsx! {
        header { class: "hidden flex-col gap-2 md:flex",
            span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                "Workspace"
            }
            div { class: "flex flex-wrap items-end justify-between gap-3",
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Projects" }
                div { class: "flex items-center gap-2",
                    if let Some(mut rel) = props.relevant {
                        button {
                            r#type: "button",
                            class: if rel() {
                                "rounded-full border border-primary/40 bg-primary/10 px-3 py-1 text-xs font-medium text-foreground transition-colors"
                            } else {
                                "rounded-full border border-border/70 bg-card/60 px-3 py-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground"
                            },
                            title: "Only projects that are active or deliberately on hold",
                            aria_pressed: rel().to_string(),
                            onclick: move |_| {
                                let v = rel();
                                rel.set(!v);
                            },
                            "Relevant"
                        }
                    }
                    if props.hidden > 0 {
                        span { class: "text-xs tabular-nums text-muted-foreground",
                            "{props.hidden} hidden"
                        }
                    }
                    if let Some(line) = &props.count_line {
                        span { class: "rounded-full border border-border/70 bg-card/60 px-3 py-1 text-xs font-medium text-muted-foreground tabular-nums",
                            "{line}"
                        }
                    }
                    if let Some(vm) = props.view_mode {
                        ViewToggle { view: vm }
                    }
                }
            }
            Text {
                variant: TextVariant::Muted,
                class: "max-w-prose",
                "Everything in flight across the org, live from the project service."
            }
            if let Some(qa) = &props.quick_add {
                {qa.clone()}
            }
        }
    }
}

#[component]
fn MobileViewToggle(props: ViewToggleProps) -> Element {
    let mut view = props.view;
    let next = match view() {
        ViewMode::Cards => ViewMode::List,
        ViewMode::List => ViewMode::Cards,
    };
    let label = match next {
        ViewMode::Cards => "Switch to cards",
        ViewMode::List => "Switch to list",
    };
    rsx! {
        button {
            r#type: "button",
            class: "pointer-events-auto flex size-8 items-center justify-center rounded-full border border-border/50 bg-background/55 text-muted-foreground/75 shadow-sm backdrop-blur transition-colors hover:bg-background hover:text-foreground",
            title: "{label}",
            aria_label: "{label}",
            onclick: move |_| view.set(next),
            match next {
                ViewMode::Cards => rsx! { LayoutGrid { size: 15 } },
                ViewMode::List => rsx! { LayoutList { size: 15 } },
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ViewToggleProps {
    view: Signal<ViewMode>,
}

#[component]
fn ViewToggle(props: ViewToggleProps) -> Element {
    let mut view = props.view;
    rsx! {
        div { class: "inline-flex items-center gap-0.5 rounded-lg bg-muted/40 p-0.5 text-xs",
            for (mode, label) in [(ViewMode::Cards, "Cards"), (ViewMode::List, "List")] {
                {
                    let active = view() == mode;
                    let cls = if active {
                        "rounded-md bg-background px-3 py-1 font-medium text-foreground shadow-sm transition-colors"
                    } else {
                        "rounded-md px-3 py-1 text-muted-foreground transition-colors hover:text-foreground"
                    };
                    rsx! {
                        button {
                            key: "{label}",
                            r#type: "button",
                            class: "{cls}",
                            onclick: move |_| view.set(mode),
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}

// ── stat tile ───────────────────────────────────────────────────────

#[derive(Props, Clone, PartialEq)]
struct StatProps {
    value: String,
    label: String,
    hint: Option<String>,
}

#[component]
fn Stat(props: StatProps) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1 bg-card p-5",
            div { class: "flex items-baseline gap-1.5",
                span { class: "text-3xl font-semibold tracking-tight tabular-nums text-foreground",
                    "{props.value}"
                }
                if let Some(h) = &props.hint {
                    span { class: "text-xs text-muted-foreground", "{h}" }
                }
            }
            span { class: "text-[0.7rem] font-medium uppercase tracking-[0.12em] text-muted-foreground",
                "{props.label}"
            }
        }
    }
}

// ── project card ────────────────────────────────────────────────────

#[derive(Props, Clone, PartialEq)]
struct ProjectCardProps {
    p: ProjectInfo,
    subprojects: Vec<ProjectInfo>,
    index: usize,
}

#[component]
fn ProjectCardView(props: ProjectCardProps) -> Element {
    let p = &props.p;
    let kids = props.subprojects.clone();
    let accent = accent(p);
    let pid = p.id.to_string();
    let progress = (p.progress_percent >= 0).then(|| p.progress_percent.clamp(0, 100));
    let summary = first_line(&p.details);
    let lead = clean_lead(&p.lead);
    let pri_label = priority_label(&p.priority);
    let pri_class = priority_class(&p.priority);
    let due = p.target_date.map(|d| d.format("%b %-d, %Y").to_string());
    let tags: Vec<String> = p.tags.0.clone();
    let shown_tags: Vec<String> = tags.iter().take(4).cloned().collect();
    let extra_tags = tags.len().saturating_sub(shown_tags.len());
    let sub_count = kids.len();
    // Staggered entrance — capped so a long list doesn't drag.
    let delay = (props.index.min(11) * 45) as u32;

    rsx! {
        div {
            class: "group relative h-full",
            style: "animation: ftsFadeUp 0.55s cubic-bezier(0.16,1,0.3,1) both; animation-delay: {delay}ms;",
            Card { class: "relative flex h-full flex-col overflow-hidden border-border/70 bg-card/70 backdrop-blur-sm transition-all duration-300 ease-out group-hover:-translate-y-1 group-hover:border-border group-hover:shadow-xl group-hover:shadow-foreground/5",
                // 16:9 cover — the project's image if set, else an
                // accent-gradient placeholder so the slot reads as
                // "ready for an image".
                // 16:9 cover. Same tile the list rows use, so a project
                // reads the same either way — an image when it has one,
                // its initials over its accent when it doesn't.
                div { class: "relative aspect-video w-full overflow-hidden bg-muted transition-transform duration-500 group-hover:scale-[1.03]",
                    ProjectTile {
                        p: props.p.clone(),
                        class: "h-full w-full".to_string(),
                        text_class: "text-4xl".to_string(),
                        rounded: String::new(),
                    }
                }
                CardHeader {
                    div { class: "flex items-start justify-between gap-3",
                        div { class: "flex min-w-0 flex-col gap-1",
                            Link {
                                to: Route::ProjectDetailRoute { id: pid.clone() },
                                class: "min-w-0",
                                CardTitle { class: "truncate transition-colors group-hover:text-foreground",
                                    "{p.title}"
                                }
                            }
                            if let Some(s) = summary {
                                CardDescription { class: "line-clamp-2", "{s}" }
                            }
                        }
                        StatusBadge { variant: status_variant(&p.status), label: p.status.clone() }
                    }
                }
                CardContent { class: "flex flex-1 flex-col",
                    div { class: "flex h-full flex-col gap-4",
                        // progress
                        if let Some(pct) = progress {
                            div { class: "flex flex-col gap-1.5",
                                div { class: "flex items-center justify-between text-xs",
                                    span { class: "text-muted-foreground", "Progress" }
                                    span { class: "font-medium tabular-nums text-foreground", "{pct}%" }
                                }
                                div { class: "h-1.5 w-full overflow-hidden rounded-full bg-muted",
                                    div {
                                        class: "h-full rounded-full transition-all duration-500",
                                        style: "width: {pct}%; background: {accent};",
                                    }
                                }
                            }
                        }
                        // metadata row
                        div { class: "flex flex-wrap items-center gap-x-4 gap-y-1.5 text-xs",
                            div { class: "flex items-center gap-1.5 {pri_class}",
                                Flag { size: 13 }
                                span { "{pri_label}" }
                            }
                            if !lead.is_empty() {
                                div { class: "flex items-center gap-1.5 text-muted-foreground",
                                    User { size: 13 }
                                    span { class: "max-w-[10rem] truncate", "{lead}" }
                                }
                            }
                            if let Some(due) = &due {
                                div { class: "flex items-center gap-1.5 text-muted-foreground",
                                    CalendarDays { size: 13 }
                                    span { "{due}" }
                                }
                            }
                            // Subproject count — a compact chip instead of the
                            // full list, so cards stay a uniform height.
                            if sub_count > 0 {
                                div { class: "flex items-center gap-1.5 text-muted-foreground",
                                    Layers { size: 13 }
                                    span {
                                        if sub_count == 1 { "1 subproject" } else { "{sub_count} subprojects" }
                                    }
                                }
                            }
                        }
                        // tags — pinned to the bottom so equal-height rows
                        // line their footers up.
                        if !shown_tags.is_empty() {
                            div { class: "mt-auto flex flex-wrap items-center gap-1.5 pt-1",
                                // Index-qualified key: frontmatter tags can repeat.
                                for (i, tag) in shown_tags.iter().enumerate() {
                                    Badge { key: "{i}-{tag}", variant: BadgeVariant::Secondary, "{tag}" }
                                }
                                if extra_tags > 0 {
                                    span { class: "text-xs text-muted-foreground", "+{extra_tags}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── identity tile ───────────────────────────────────────────────────

/// Project id → the slug of the org that owns it, for signing cover
/// URLs. Provided by the page, read by [`ProjectTile`].
type CoverOrgs = std::collections::HashMap<uuid::Uuid, String>;

/// The project's face: its image, or its initials over its own accent.
///
/// Both layouts render this, which is the point — a project should be
/// recognizable whichever way you are looking at the page. It replaced
/// the list view's 1px accent rail, which carried a colour but no
/// information; a tile carries identity, and the same pixels now do
/// work instead of decoration.
///
/// The fallback is deliberately typographic rather than a generic
/// folder glyph. Every project without an image used to look like
/// every other one; initials over the project's accent are unique per
/// project and readable at 40px.
#[derive(Props, Clone, PartialEq)]
struct ProjectTileProps {
    p: ProjectInfo,
    /// Tailwind size classes — `size-10` in rows, `h-full w-full` for
    /// a card cover.
    class: String,
    /// Initials size. Rows want `text-xs`; covers want something big.
    text_class: String,
    rounded: String,
}

#[component]
fn ProjectTile(props: ProjectTileProps) -> Element {
    let p = &props.p;
    let accent = accent(p);
    let initials = initials(&p.title);
    let cls = format!(
        "{} {} relative shrink-0 overflow-hidden",
        props.class, props.rounded
    );

    // A vault-hosted cover needs a signed grant before it will load;
    // an external URL must not be rewritten. `cover_src` resolves both,
    // and returns `None` until a grant arrives — so the initials show
    // in the meantime rather than a broken image.
    let image = p.image.trim().to_owned();
    let pid = p.id;
    let cover_orgs = use_context::<Memo<CoverOrgs>>();
    let src = use_resource(move || {
        let image = image.clone();
        let slug = cover_orgs.read().get(&pid).cloned();
        async move { cover_src(&image, slug.as_deref()).await }
    });
    let resolved = src.read().clone().flatten();

    rsx! {
        div { class: "{cls}",
            if let Some(url) = resolved {
                img {
                    src: "{url}",
                    alt: "",
                    loading: "lazy",
                    class: "h-full w-full object-cover",
                }
            } else {
                // Two stops of the same accent: enough shape to read as
                // a deliberate tile, quiet enough not to shout in a
                // list of twenty.
                div {
                    class: "flex h-full w-full items-center justify-center",
                    style: "background: linear-gradient(140deg, color-mix(in oklch, {accent} 82%, transparent) 0%, color-mix(in oklch, {accent} 28%, var(--card)) 100%);",
                    span {
                        class: "{props.text_class} font-semibold tracking-tight text-background mix-blend-luminosity",
                        "{initials}"
                    }
                }
            }
        }
    }
}

/// Is this cover an address the browser can already fetch?
///
/// Only these four schemes. Anything else — `projects/x.jpg`,
/// `/org/…`, `Resources/covers/x.png` — is a path inside the org's own
/// media tree, which is not public and must be signed.
fn is_absolute_url(src: &str) -> bool {
    ["http://", "https://", "data:", "blob:"]
        .iter()
        .any(|s| src.starts_with(s))
}

/// The URL to actually put in `<img src>`, or `None` when there is no
/// usable cover.
///
/// Two kinds of value live in `project.image`, and conflating them is
/// what makes covers silently 401:
///
/// - an **absolute URL** — used verbatim, because rewriting somebody's
///   `https://…` into a media path would break it;
/// - anything else — a path inside the org's media tree, served by
///   `GET /org/{slug}/media/{path}`, which since
///   `TASK_ENFORCE_MEDIA_TOKEN` requires a short-lived signed grant.
///
/// The grant is minted over the path's PARENT directory, so one grant
/// covers every cover in a folder rather than one per project.
///
/// Failure is `None`, not a broken URL: an unsigned request would 401
/// and render as a broken-image glyph, which reads as a bug. Falling
/// back to the initials tile reads as a project without a picture.
async fn cover_src(image: &str, slug: Option<&str>) -> Option<String> {
    if image.is_empty() {
        return None;
    }
    if is_absolute_url(image) {
        return Some(image.to_owned());
    }
    // The owning org, not the active one: a project from another org in
    // the same fan-out must be signed against ITS server.
    let slug = slug?;
    let path = image.trim_start_matches('/');
    let prefix = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let suffix = task_player_ui::media_grant::suffix_for_prefix(slug, prefix).await;
    if suffix.is_empty() {
        // No grant — the request would be rejected, so don't make it.
        return None;
    }
    Some(format!("/org/{slug}/media/{path}{suffix}"))
}

/// Up to two initials from a title. `PNG Worship Collective Album` →
/// `PW`; `Task` → `T`. Leading numbers are skipped — a studio archive
/// is full of `01 Michael Kaplan Sessions`, and `0M` identifies
/// nothing.
fn initials(title: &str) -> String {
    let words: Vec<&str> = title
        .split_whitespace()
        .filter(|w| w.chars().next().is_some_and(char::is_alphabetic))
        .collect();
    let mut out = String::new();
    for w in words.iter().take(2) {
        if let Some(c) = w.chars().next() {
            out.extend(c.to_uppercase());
        }
    }
    if out.is_empty() {
        title
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "·".into())
    } else {
        out
    }
}

// ── project row (list view) ─────────────────────────────────────────

#[derive(Props, Clone, PartialEq)]
struct ProjectRowProps {
    p: ProjectInfo,
    sub_count: usize,
    first: bool,
}

#[component]
fn ProjectRow(props: ProjectRowProps) -> Element {
    let p = &props.p;
    let accent = accent(p);
    let pid = p.id.to_string();
    let progress = (p.progress_percent >= 0).then(|| p.progress_percent.clamp(0, 100));
    let pri_label = priority_label(&p.priority);
    let pri_class = priority_class(&p.priority);
    let shown_tags: Vec<String> = p.tags.0.iter().take(3).cloned().collect();
    let row_cls = if props.first {
        "group flex items-center gap-3.5 bg-card/40 px-3 py-2.5 transition-colors hover:bg-accent/30"
    } else {
        "group flex items-center gap-3.5 border-t border-border/60 bg-card/40 px-3 py-2.5 transition-colors hover:bg-accent/30"
    };

    rsx! {
        Link { to: Route::ProjectDetailRoute { id: pid }, class: "{row_cls}",
            // Identity tile — the row's anchor. Replaces the old accent
            // rail: same colour information, plus the project's image
            // and a readable fallback.
            ProjectTile {
                p: props.p.clone(),
                class: "size-10".to_string(),
                text_class: "text-xs".to_string(),
                rounded: "rounded-lg".to_string(),
            }
            // title + inline meta
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                span { class: "truncate text-sm font-medium text-foreground transition-colors group-hover:text-foreground",
                    "{p.title}"
                }
                div { class: "flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-xs text-muted-foreground",
                    span { class: "{pri_class}", "{pri_label}" }
                    if props.sub_count > 0 {
                        span { "· {props.sub_count} sub" }
                    }
                    // Index-qualified key: frontmatter tags can repeat.
                    for (i, tag) in shown_tags.iter().enumerate() {
                        span { key: "{i}-{tag}", class: "hidden sm:inline", "· {tag}" }
                    }
                }
            }
            // progress (desktop)
            if let Some(pct) = progress {
                div { class: "hidden w-32 shrink-0 items-center gap-2 md:flex",
                    div { class: "h-1.5 flex-1 overflow-hidden rounded-full bg-muted",
                        div {
                            class: "h-full rounded-full",
                            style: "width: {pct}%; background: {accent};",
                        }
                    }
                    span { class: "w-8 shrink-0 text-right text-xs tabular-nums text-muted-foreground",
                        "{pct}%"
                    }
                }
            }
            StatusBadge { variant: status_variant(&p.status), label: p.status.clone() }
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────────

/// Accent for a project: its own stored `color` if set, else a stable
/// pick from the chart palette by title hash. Always a CSS value safe
/// for an inline `style` (a `var(--chart-N)` token, or user-set data).
fn accent(p: &ProjectInfo) -> String {
    if p.color.trim().is_empty() {
        let n = 1 + (fnv1a(&p.title) % 5);
        format!("var(--chart-{n})")
    } else {
        p.color.clone()
    }
}

/// Tiny deterministic hash so a project's accent is stable across loads.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

enum Bucket {
    Active,
    Hold,
}

fn matches_status(status: &str, bucket: Bucket) -> bool {
    match bucket {
        Bucket::Active => matches!(status, "active" | "open" | "in_progress"),
        Bucket::Hold => matches!(status, "on_hold" | "on-hold" | "paused" | "waiting"),
    }
}

fn priority_label(pr: &str) -> String {
    match pr {
        "p0" | "urgent" => "Urgent".into(),
        "p1" | "high" => "High".into(),
        "" | "p2" | "normal" => "Normal".into(),
        "p3" | "low" => "Low".into(),
        "p4" | "lowest" => "Lowest".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
    }
}

fn priority_class(pr: &str) -> &'static str {
    match pr {
        "p0" | "urgent" | "p1" | "high" => "text-destructive",
        "p3" | "low" | "p4" | "lowest" => "text-muted-foreground/60",
        _ => "text-muted-foreground",
    }
}

/// Strip a `[[wikilink]]` wrapper from a lead for display.
fn clean_lead(s: &str) -> String {
    s.trim()
        .trim_start_matches("[[")
        .trim_end_matches("]]")
        .trim()
        .to_string()
}

/// First meaningful line of the markdown body as a plain-text card
/// summary: skips front-matter rules and ATX headings, and strips
/// leading markup (`#`, `>`, list bullets) from the chosen line.
fn first_line(body: &str) -> Option<String> {
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("---") && !l.starts_with('#'))?;
    let cleaned = line.trim_start_matches(['#', '>', '-', '*', ' ']).trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}
