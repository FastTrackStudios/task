//! `/finances` — a billing dashboard rolled up from the timer's work
//! sessions across the selected org(s). Read-only: total / billable
//! hours, billable revenue, and breakdowns by project and by org for a
//! chosen period. The finance service isn't mounted over vox yet, so
//! everything here is computed from the (mounted) timer `list_sessions`.

use crate::format::money;
use std::collections::HashMap;

use architect_ui::prelude::*;
use chrono::{Datelike, Utc};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};

#[derive(Clone, Copy, PartialEq)]
enum Period {
    Week,
    Month,
    All,
}

impl Period {
    fn label(self) -> &'static str {
        match self {
            Self::Week => "Last 7 days",
            Self::Month => "This month",
            Self::All => "All time",
        }
    }
}

fn hours(secs: i64) -> String {
    format!("{:.1}h", secs as f64 / 3600.0)
}

#[component]
pub fn FinancesView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let mut period = use_signal(|| Period::All);

    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));

    let sessions =
        use_resource(move || async move { crate::feeds::fetch_sessions_multi(&slugs()).await });
    // Project id → title, for the by-project breakdown.
    let projects = use_resource(move || async move {
        crate::feeds::fetch_projects(&slugs())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.id, p.title))
            .collect::<HashMap<Uuid, String>>()
    });

    let rows = sessions.read().clone().unwrap_or_default();
    let proj_names = projects.read().clone().unwrap_or_default();

    // Period cutoff (rolling windows; All = no cutoff).
    let today = Utc::now().date_naive();
    let in_period = |d: chrono::NaiveDate| match period() {
        Period::All => true,
        Period::Week => (today - d).num_days() < 7,
        Period::Month => d.year() == today.year() && d.month() == today.month(),
    };

    // Roll up closed sessions in the period.
    let mut total_secs = 0i64;
    let mut billable_secs = 0i64;
    let mut billable_cents = 0i64;
    let mut count = 0usize;
    let mut by_project: HashMap<Option<Uuid>, (i64, i64)> = HashMap::new();
    let mut by_org: HashMap<String, (i64, i64)> = HashMap::new();
    for (slug, s) in &rows {
        let Some(end) = s.end_time else { continue };
        if !in_period(s.start_time.date_naive()) {
            continue;
        }
        let secs = (end - s.start_time).num_seconds().max(0);
        let cents = if s.billable {
            secs * s.rate_cents / 3600
        } else {
            0
        };
        total_secs += secs;
        count += 1;
        if s.billable {
            billable_secs += secs;
            billable_cents += cents;
        }
        let p = by_project.entry(s.project_id).or_default();
        p.0 += secs;
        p.1 += cents;
        let o = by_org.entry(slug.clone()).or_default();
        o.0 += secs;
        o.1 += cents;
    }

    // Sort breakdowns by revenue desc.
    let mut proj_rows: Vec<(String, i64, i64)> = by_project
        .into_iter()
        .map(|(pid, (sec, cent))| {
            let name = pid.map_or_else(
                || "Unassigned".to_string(),
                |id| {
                    proj_names
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| "Unknown project".into())
                },
            );
            (name, sec, cent)
        })
        .collect();
    proj_rows.sort_by(|a, b| b.2.cmp(&a.2));
    let mut org_rows: Vec<(String, i64, i64)> = by_org
        .into_iter()
        .map(|(slug, (sec, cent))| {
            let name = org_list
                .read()
                .iter()
                .find(|o| o.slug == slug)
                .map_or(slug.clone(), |o| o.name.clone());
            (name, sec, cent)
        })
        .collect();
    org_rows.sort_by(|a, b| b.2.cmp(&a.2));

    let loading = sessions.read().is_none();

    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-8",
            header { class: "flex items-center justify-between gap-3",
                div { class: "flex flex-col gap-1",
                    span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                        "Billing"
                    }
                    Heading { level: HeadingLevel::H1, class: "tracking-tight", "Finances" }
                }
                div { class: "flex gap-1",
                    for p in [Period::Week, Period::Month, Period::All] {
                        Button {
                            key: "{p.label()}",
                            variant: if period() == p { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                            size: ButtonSize::Small,
                            on_click: move |_| period.set(p),
                            "{p.label()}"
                        }
                    }
                }
            }

            if loading {
                Text { variant: TextVariant::Muted, "Loading…" }
            } else {
                // Summary cards.
                div { class: "grid grid-cols-2 gap-3 sm:grid-cols-4",
                    StatCard { label: "Billable", value: money(billable_cents), accent: true }
                    StatCard { label: "Billable hrs", value: hours(billable_secs), accent: false }
                    StatCard { label: "Total hrs", value: hours(total_secs), accent: false }
                    StatCard { label: "Sessions", value: "{count}".to_string(), accent: false }
                }

                // By project.
                if !proj_rows.is_empty() {
                    Breakdown { title: "By project".to_string(), rows: proj_rows }
                }
                // By org (only meaningful across multiple).
                if org_rows.len() > 1 {
                    Breakdown { title: "By organization".to_string(), rows: org_rows }
                }

                if count == 0 {
                    div { class: "rounded-lg border border-dashed border-border px-4 py-10 text-center",
                        Text { variant: TextVariant::Muted, "No tracked time in this period." }
                    }
                }
            }
        }
    }
}

#[component]
fn StatCard(label: String, value: String, accent: bool) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1 rounded-xl border border-border/60 bg-card/40 p-3",
            span { class: "text-xs text-muted-foreground", "{label}" }
            span {
                class: if accent {
                    "font-mono text-xl font-semibold tabular-nums text-emerald-400"
                } else {
                    "font-mono text-xl font-semibold tabular-nums text-foreground"
                },
                "{value}"
            }
        }
    }
}

#[component]
fn Breakdown(title: String, rows: Vec<(String, i64, i64)>) -> Element {
    rsx! {
        div { class: "flex flex-col gap-2",
            Heading { level: HeadingLevel::H3, "{title}" }
            div { class: "flex flex-col divide-y divide-border/50 rounded-xl border border-border/60 bg-card/40",
                for (name , secs , cents) in rows {
                    div { key: "{name}", class: "flex items-center justify-between gap-3 px-3 py-2.5",
                        span { class: "truncate text-sm text-foreground", "{name}" }
                        div { class: "flex shrink-0 items-center gap-4",
                            span { class: "font-mono text-xs tabular-nums text-muted-foreground", "{hours(secs)}" }
                            span { class: "font-mono text-sm tabular-nums text-foreground", "{money(cents)}" }
                        }
                    }
                }
            }
        }
    }
}
