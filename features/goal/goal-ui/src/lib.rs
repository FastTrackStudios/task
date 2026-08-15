//! `/goals` — goal hierarchy + cycle anchoring.
//!
//! Loads the selected org(s)' goals via the multi-org
//! [`task_ui_core::feeds::fan_out`] (the same convention as `/projects` and
//! `/tasks`): "All" aggregates every hosted org, selecting one scopes
//! to it. Cycle pills resolve `cycle_id` ⇒ `(year, quarter, ordinal)`
//! via the local pure-function generator (deterministic UUIDv5 means
//! the wasm side agrees with the server's seeded files).

use architect_ui::prelude::*;
use chrono::Weekday;
use cycle::{FirstWeekRule, generate_year};
use dioxus::prelude::*;
use goal_proto::Goal;
use uuid::Uuid;

use task_ui_core::orgs::{OrgMeta, OrgSelection};

/// Self-fetching `/goals` page — resolves the org selection, fans
/// out its own `GoalService` calls, and renders [`GoalsScreen`].
/// Shells that own a live goal store (the app shell's `stores!`
/// layer) skip this and drive [`GoalsScreen`] directly.
#[component]
pub fn GoalsView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let goals = use_resource(move || async move {
        let slugs = task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read());
        fetch_goals(&slugs).await
    });

    // While the org list is still being discovered the fetch resolves
    // to an empty set — show loading rather than an empty hierarchy
    // (mirrors the projects/home gate).
    let (rows, error) = if org_list.read().is_empty() {
        (None, None)
    } else {
        match &*goals.read_unchecked() {
            Some(Ok(rows)) => (Some(rows.clone()), None),
            Some(Err(e)) => (None, Some(e.clone())),
            None => (None, None),
        }
    };
    rsx! {
        GoalsScreen { rows, error }
    }
}

/// Props for [`GoalsScreen`] — the presentational goals page.
#[derive(Props, Clone, PartialEq)]
pub struct GoalsScreenProps {
    /// Merged rows across the selected orgs; `None` while loading.
    pub rows: Option<Vec<Goal>>,
    /// Fetch error to surface instead of rows.
    #[props(default)]
    pub error: Option<String>,
}

/// The goals page chrome + hierarchy, rendered from caller-owned
/// rows — the store-driven shell hands its live goal list in here.
#[component]
pub fn GoalsScreen(props: GoalsScreenProps) -> Element {
    // Current cycle highlight — drives the "you're here" pill.
    let today = chrono::Local::now().date_naive();
    let now_cycle =
        cycle::cycle_for_date(today, Weekday::Mon, FirstWeekRule::AtLeastFourDaysInYear);

    let body = if let Some(e) = &props.error {
        rsx! {
            div { class: "rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm",
                "Couldn't reach the goal service: {e}"
            }
        }
    } else if let Some(rows) = &props.rows {
        render_loaded(rows, now_cycle.clone())
    } else {
        rsx! { Text { variant: TextVariant::Muted, "Loading goals…" } }
    };

    rsx! {
        div { class: "mx-auto w-full max-w-5xl flex flex-col gap-6 p-4 sm:p-6 lg:p-10",
            div { class: "flex items-start justify-between gap-4 flex-wrap",
                div { class: "flex flex-col gap-1",
                    Heading { level: HeadingLevel::H1, "Goals" }
                    Text {
                        variant: TextVariant::Muted,
                        "Live from the selected orgs' goal services."
                    }
                }
                if let Some(c) = now_cycle.clone() {
                    div { class: "rounded-lg border border-border/40 bg-muted/20 px-3 py-2 text-xs",
                        Text { variant: TextVariant::Muted, class: "uppercase tracking-wider text-[10px]", "Now" }
                        div { class: "font-medium text-sm tabular-nums",
                            "{c.year} Q{c.quarter} C{c.ordinal}"
                        }
                        div { class: "text-muted-foreground tabular-nums",
                            "{c.start_date} → {c.end_date}"
                        }
                    }
                }
            }
            {body}
        }
    }
}

fn render_loaded(rows: &[Goal], now_cycle: Option<cycle::Cycle>) -> Element {
    if rows.is_empty() {
        return rsx! {
            Text {
                variant: TextVariant::Muted,
                "No goals found. Seed a `type: goal` page under `vault/Goals/`."
            }
        };
    }
    let lifetime_roots: Vec<&Goal> = rows.iter().filter(|g| g.parent_id.is_none()).collect();
    let total = rows.len();
    let cycle_anchored = rows.iter().filter(|g| g.cycle_id.is_some()).count();
    let all_rows: Vec<Goal> = rows.to_vec();

    rsx! {
        div { class: "flex flex-col gap-4",
            Text {
                variant: TextVariant::Muted,
                class: "text-xs",
                "{total} goals · {cycle_anchored} anchored to a cycle"
            }
            for root in lifetime_roots.iter() {
                {
                    let root: Goal = (*root).clone();
                    rsx! {
                        GoalBranch {
                            key: "{root.id}",
                            g: root,
                            all_goals: all_rows.clone(),
                            now_cycle: now_cycle.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct GoalBranchProps {
    g: Goal,
    all_goals: Vec<Goal>,
    now_cycle: Option<cycle::Cycle>,
}

#[component]
fn GoalBranch(props: GoalBranchProps) -> Element {
    let g = props.g.clone();
    let descendants: Vec<Goal> = collect_descendants(g.id, &props.all_goals);
    let now = props.now_cycle.clone();
    let all = props.all_goals.clone();
    let title = g.title.clone();
    let kind = g.kind.clone();
    let status = g.status.clone();
    let summary = first_line(&g.details);
    let target = g.target_date.map(|d| d.to_string());

    rsx! {
        Card {
            CardHeader {
                div { class: "flex items-start justify-between gap-2 flex-wrap",
                    CardTitle { "{title}" }
                    div { class: "flex items-center gap-1.5",
                        KindChip { kind: kind.clone() }
                        StatusBadge {
                            variant: status_variant(&status),
                            label: status.clone(),
                        }
                    }
                }
                if let Some(s) = summary {
                    CardDescription { "{s}" }
                }
            }
            CardContent {
                if let Some(td) = target {
                    Text { variant: TextVariant::Muted, class: "text-xs",
                        "Target: {td}"
                    }
                }
                if !descendants.is_empty() {
                    div { class: "mt-3 flex flex-col gap-1.5",
                        for d in descendants.iter() {
                            {
                                let d_clone: Goal = d.clone();
                                rsx! {
                                    GoalRow {
                                        key: "{d_clone.id}",
                                        g: d_clone,
                                        all_goals: all.clone(),
                                        now_cycle: now.clone(),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct GoalRowProps {
    g: Goal,
    all_goals: Vec<Goal>,
    now_cycle: Option<cycle::Cycle>,
}

#[component]
fn GoalRow(props: GoalRowProps) -> Element {
    let g = props.g.clone();
    let depth = depth_of(g.id, &props.all_goals);
    let indent_px = (depth as u32) * 16;
    let cycle_label = cycle_label_for(&g);
    let is_current_cycle = matches!(
        (&cycle_label, &props.now_cycle),
        (Some((y, q, o)), Some(c)) if *y == c.year && *q == c.quarter && *o == c.ordinal
    );
    let row_cls = if is_current_cycle {
        "flex items-center justify-between gap-2 rounded-md border border-primary/40 bg-primary/[0.04] px-3 py-2 text-sm"
    } else {
        "flex items-center justify-between gap-2 rounded-md bg-muted/30 px-3 py-2 text-sm"
    };
    let title = g.title.clone();
    let kind = g.kind.clone();
    let status = g.status.clone();
    let target = g.target_date.map(|d| d.to_string());

    rsx! {
        div {
            class: "{row_cls}",
            style: "margin-left: {indent_px}px;",
            div { class: "flex flex-col min-w-0 gap-0.5",
                span { class: "font-medium truncate", "{title}" }
                if let Some((y, q, o)) = cycle_label {
                    span {
                        class: if is_current_cycle {
                            "text-xs text-primary font-medium tabular-nums"
                        } else {
                            "text-xs text-muted-foreground tabular-nums"
                        },
                        "{y} Q{q} C{o}"
                        if is_current_cycle { " · current cycle" }
                    }
                } else if let Some(td) = target {
                    span { class: "text-xs text-muted-foreground tabular-nums",
                        "target {td}"
                    }
                }
            }
            div { class: "flex items-center gap-1.5 shrink-0",
                KindChip { kind: kind.clone() }
                StatusBadge {
                    variant: status_variant(&status),
                    label: status.clone(),
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct KindChipProps {
    kind: String,
}

#[component]
fn KindChip(props: KindChipProps) -> Element {
    let cls = match props.kind.as_str() {
        "lifetime" => "rounded-full px-2 py-0.5 bg-accent/30 text-accent-foreground text-[11px]",
        "yearly" => "rounded-full px-2 py-0.5 bg-primary/10 text-primary text-[11px]",
        "quarterly" => "rounded-full px-2 py-0.5 bg-muted/60 text-muted-foreground text-[11px]",
        "cycle" => "rounded-full px-2 py-0.5 bg-success/15 text-success text-[11px]",
        _ => "rounded-full px-2 py-0.5 bg-muted/40 text-muted-foreground text-[11px]",
    };
    let label = props.kind;
    rsx! { span { class: "{cls}", "{label}" } }
}

fn collect_descendants(parent_id: Uuid, all: &[Goal]) -> Vec<Goal> {
    let mut out: Vec<Goal> = Vec::new();
    for g in all.iter().filter(|g| g.parent_id == Some(parent_id)) {
        out.push(g.clone());
        out.extend(collect_descendants(g.id, all));
    }
    out
}

fn depth_of(id: Uuid, all: &[Goal]) -> usize {
    let mut depth = 0usize;
    let mut current = id;
    loop {
        let Some(g) = all.iter().find(|g| g.id == current) else {
            break;
        };
        match g.parent_id {
            Some(p) => {
                depth += 1;
                current = p;
            }
            None => break,
        }
    }
    depth.saturating_sub(1)
}

fn status_variant(status: &str) -> StatusBadgeVariant {
    match status {
        "active" | "achieved" => StatusBadgeVariant::Success,
        "paused" | "on-hold" => StatusBadgeVariant::Warning,
        "abandoned" | "cancelled" => StatusBadgeVariant::Danger,
        _ => StatusBadgeVariant::Neutral,
    }
}

/// Resolve a goal's `cycle_id` (or `target_date`) to a
/// `(year, quarter, ordinal)` triple by scanning the
/// generator's deterministic UUIDv5 ids. We walk a window
/// of nearby years to cover dates straddling the cyclic
/// new year.
fn cycle_label_for(g: &Goal) -> Option<(i32, u8, u8)> {
    use chrono::Datelike;
    let id = g.cycle_id?;
    let base = chrono::Local::now().date_naive().year();
    for year_offset in [-1, 0, 1, 2] {
        let qs = generate_year(
            base + year_offset,
            Weekday::Mon,
            FirstWeekRule::AtLeastFourDaysInYear,
        );
        for q in qs {
            for c in q.cycles.iter() {
                if c.id == id {
                    return Some((c.year, c.quarter, c.ordinal));
                }
            }
        }
    }
    None
}

fn first_line(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("---"))
        .map(std::string::ToString::to_string)
}

// ── data ────────────────────────────────────────────────────────────
//
// This slice's RPCs live with the page that calls them, not in the
// shell's `feeds` module — that is the point of the split. `feeds!` and
// the fan-out helpers come from `task-ui-core`; see its `feeds` module
// for the shape.

/// Goals across the selected orgs (concurrent fan-out).
pub async fn fetch_goals(slugs: &[String]) -> Result<Vec<goal_proto::Goal>, String> {
    task_ui_core::feeds::fan_out(
        slugs,
        "list",
        |c: goal_proto::GoalServiceClient| async move { c.list().await },
    )
    .await
}
