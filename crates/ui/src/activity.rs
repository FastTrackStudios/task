//! The activity feed, as the app shows it.
//!
//! One history, three windows on it:
//!
//! - [`ActivityPage`] — `/activity`: the whole org's feed, newest first,
//!   grouped by day (Today / Yesterday / Mon, Oct 12), scrolling back a
//!   page at a time; ⋯ → Filter by kind group and project, "Only mine".
//! - [`EntityActivity`] — one thing's timeline, oldest first, Linear's
//!   issue-page shape: "Alice changed status from Open to In Progress ·
//!   2h", comments inline, and (for a task) a composer at the bottom.
//!   The project page mounts it with `project: true`, which takes in
//!   the project's tasks too.
//!
//! Every row comes from the shared [`crate::stores::ActivityStore`] (the
//! newest page plus the live fold) merged with the pages this surface
//! fetched for itself (a filter's first page, scroll-back, an entity's
//! history) — so a change made anywhere appears here as it happens,
//! and an older page is never lost to a store reload.
//!
//! The words are [`describe`] / [`describe_here`] — pure, unit-tested.
//! Spec: `docs/spec/activity.md`.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use dioxus::prelude::*;
use notify_proto::{Activity, ActivityCursor, ActivityFilter, ActivityGroup, ActivityKind};
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

// ── words ────────────────────────────────────────────────────────────

/// A status / priority value as a person reads it: `in-progress` →
/// `In Progress`.
#[must_use]
pub fn value_label(raw: &str) -> String {
    raw.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase().collect::<String>() + c.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A due date as a person reads it: `2026-10-20` → `Oct 20`.
#[must_use]
pub fn date_label(raw: &str) -> String {
    let day = raw.get(..10).unwrap_or(raw);
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map_or_else(|_| raw.to_owned(), |d| d.format("%b %-d").to_string())
}

/// One feed line, in three parts: what was done (`moved`), to what
/// (`Comp lead vocals`), and how (`to In Progress`). The actor leads,
/// separately, so the UI can set it apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub verb: String,
    pub object: String,
    pub detail: String,
}

fn s(verb: &str, object: &str, detail: impl Into<String>) -> Sentence {
    Sentence {
        verb: verb.to_owned(),
        object: object.to_owned(),
        detail: detail.into(),
    }
}

/// The `/activity` sentence: names the thing it is about.
// t[impl activity.ui.sentence]
#[must_use]
pub fn describe(a: &Activity) -> Sentence {
    let title = if a.entity_title.trim().is_empty() {
        "an item"
    } else {
        a.entity_title.as_str()
    };
    match a.kind {
        ActivityKind::Created => s("created", title, ""),
        ActivityKind::StatusChanged => s("moved", title, format!("to {}", value_label(&a.to))),
        ActivityKind::Completed => s("completed", title, ""),
        ActivityKind::Reopened => s("reopened", title, ""),
        ActivityKind::Assigned if a.to == a.actor_name => s("took", title, ""),
        ActivityKind::Assigned => s("assigned", title, format!("to {}", a.to)),
        ActivityKind::Unassigned => s("unassigned", title, format!("from {}", a.from)),
        ActivityKind::PriorityChanged => s(
            "set the priority of",
            title,
            format!("to {}", value_label(&a.to)),
        ),
        ActivityKind::DueChanged if a.to.is_empty() => s("cleared the due date of", title, ""),
        ActivityKind::DueChanged => s("set", title, format!("due {}", date_label(&a.to))),
        ActivityKind::Renamed => s("renamed", &a.from, format!("to “{}”", a.to)),
        ActivityKind::Edited => s("edited the description of", title, ""),
        ActivityKind::Deleted => s("deleted", title, ""),
        ActivityKind::Commented => s("commented on", title, ""),
        ActivityKind::Captured => s("captured", title, ""),
        ActivityKind::Processed if a.to.is_empty() => s("processed", title, ""),
        ActivityKind::Processed => s(
            "processed",
            title,
            format!(
                "into {} “{}”",
                if a.field.is_empty() { "a" } else { &a.field },
                a.to
            ),
        ),
        ActivityKind::TimerStarted => s("started a timer on", title, ""),
        ActivityKind::TimerStopped => s("stopped the timer on", title, after(&a.body)),
        ActivityKind::ModeStarted => s("started", title, ""),
        ActivityKind::ModeEnded => s("ended", title, after(&a.body)),
        ActivityKind::ProjectCreated => s("created the project", title, ""),
        ActivityKind::ProjectUpdated if a.field == "title" => {
            s("renamed the project", &a.from, format!("to “{}”", a.to))
        }
        ActivityKind::ProjectUpdated => s(
            "moved the project",
            title,
            format!("to {}", value_label(&a.to)),
        ),
        ActivityKind::MilestoneCreated => s("created the milestone", title, ""),
        ActivityKind::MilestoneUpdated => s("updated the milestone", title, ""),
        ActivityKind::Other => s("updated", title, ""),
    }
}

fn after(body: &str) -> String {
    if body.is_empty() {
        String::new()
    } else {
        format!("after {body}")
    }
}

/// The timeline sentence on the thing's own page — no title (you are
/// looking at it), Linear's phrasing: "changed status from Open to In
/// Progress".
#[must_use]
pub fn describe_here(a: &Activity, here: &str) -> String {
    let what = match a.entity_type.as_str() {
        "project" => "the project",
        "task" => "the task",
        _ => "this",
    };
    // A project's timeline carries its tasks' lines too: those keep
    // their title.
    if a.entity_id != here {
        let w = describe(a);
        return [w.verb, w.object, w.detail]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }
    match a.kind {
        ActivityKind::Created | ActivityKind::ProjectCreated => format!("created {what}"),
        ActivityKind::StatusChanged | ActivityKind::Completed | ActivityKind::Reopened => {
            format!(
                "changed status from {} to {}",
                value_label(&a.from),
                value_label(&a.to)
            )
        }
        ActivityKind::Assigned if a.to == a.actor_name => format!("self-assigned {what}"),
        ActivityKind::Assigned => format!("assigned {what} to {}", a.to),
        ActivityKind::Unassigned => format!("removed {} from {what}", a.from),
        ActivityKind::PriorityChanged => format!("set priority to {}", value_label(&a.to)),
        ActivityKind::DueChanged if a.to.is_empty() => "removed the due date".to_owned(),
        ActivityKind::DueChanged => format!("set the due date to {}", date_label(&a.to)),
        ActivityKind::Renamed => format!("changed the title to “{}”", a.to),
        ActivityKind::Edited => "edited the description".to_owned(),
        ActivityKind::Deleted => format!("deleted {what}"),
        ActivityKind::Commented => "commented".to_owned(),
        ActivityKind::ProjectUpdated if a.field == "title" => {
            format!("renamed the project to “{}”", a.to)
        }
        ActivityKind::ProjectUpdated => format!("moved the project to {}", value_label(&a.to)),
        _ => {
            let w = describe(a);
            [w.verb, w.object, w.detail]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// The day heading a row sits under: "Today", "Yesterday", "Mon, Oct
/// 12" (with the year once it is not this one).
#[must_use]
pub fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        "Today".to_owned()
    } else if Some(day) == today.pred_opt() {
        "Yesterday".to_owned()
    } else if day.year() == today.year() {
        day.format("%a, %b %-d").to_string()
    } else {
        day.format("%a, %b %-d, %Y").to_string()
    }
}

/// "now", "5m", "3h", "2d", then the date.
#[must_use]
pub fn rel_time(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - t).num_seconds().max(0);
    match secs {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        86_400..604_800 => format!("{}d", secs / 86_400),
        _ => t.with_timezone(&Local).format("%b %-d").to_string(),
    }
}

/// Rows newest first, grouped under their local day's heading.
#[must_use]
pub fn group_by_day(rows: &[Activity], today: NaiveDate) -> Vec<(String, Vec<Activity>)> {
    let mut out: Vec<(String, Vec<Activity>)> = Vec::new();
    for a in rows {
        let label = day_label(a.at.with_timezone(&Local).date_naive(), today);
        match out.last_mut() {
            Some((l, v)) if *l == label => v.push(a.clone()),
            _ => out.push((label, vec![a.clone()])),
        }
    }
    out
}

/// The union of several row sets, one row per id, newest first.
#[must_use]
pub fn merge_newest_first(sets: impl IntoIterator<Item = Vec<Activity>>) -> Vec<Activity> {
    let mut by_id: HashMap<Uuid, Activity> = HashMap::new();
    for set in sets {
        for a in set {
            by_id.insert(a.id, a);
        }
    }
    let mut rows: Vec<Activity> = by_id.into_values().collect();
    rows.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    rows
}

// ── shared bits ──────────────────────────────────────────────────────

fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// A small round initial — the byline's face.
#[component]
fn Avatar(name: String, #[props(default = false)] small: bool) -> Element {
    let size = if small {
        "size-5 text-[10px]"
    } else {
        "size-7 text-[12px]"
    };
    rsx! {
        span {
            class: "flex {size} shrink-0 items-center justify-center rounded-full bg-foreground/[0.08] font-semibold text-foreground/80",
            aria_hidden: "true",
            "{initial(&name)}"
        }
    }
}

/// The first selected org's `(slug, id)`, like every register page.
fn use_target_org() -> Memo<Option<(String, Option<Uuid>)>> {
    let selection = use_context::<Signal<OrgSelection>>();
    let orgs = use_context::<Signal<Vec<OrgMeta>>>();
    use_memo(move || {
        let slugs = task_ui_core::orgs::selected_slugs(&selection.read(), &orgs.read());
        let slug = slugs.into_iter().next()?;
        let id = orgs
            .read()
            .iter()
            .find(|o| o.slug == slug)
            .and_then(|o| o.id);
        Some((slug, id))
    })
}

// ── /activity ────────────────────────────────────────────────────────

/// `/activity` — the org's whole history. See the module docs.
// t[impl activity.ui.page]
#[component]
pub fn ActivityPage() -> Element {
    let target = use_target_org();
    let account = use_context::<crate::auth::AuthCtx>().active;
    let store = stores::use_activity_store();
    let _live = stores::use_activity_list();
    let projects = stores::use_project_list();
    let nav = use_navigator();

    // Filters (empty set = everything).
    let mut groups = use_signal(HashSet::<ActivityGroup>::new);
    let mut project_ids = use_signal(HashSet::<Uuid>::new);
    let mut only_mine = use_signal(|| false);
    let mut sheet = use_signal(|| false);
    let mut query = use_signal(String::new);

    let me = use_memo(move || account.read().as_ref().map(|a| a.user_id.to_string()));
    let filter = use_memo(move || ActivityFilter {
        kinds: groups.read().iter().flat_map(|g| g.kinds()).collect(),
        // One project server-side; several are narrowed client-side
        // over the union of their pages.
        project_id: (project_ids.read().len() == 1)
            .then(|| project_ids.read().iter().next().copied())
            .flatten(),
        actor_id: if only_mine() { me() } else { None },
        limit: Some(50),
        ..ActivityFilter::default()
    });

    // Pages this page fetched: the filter's first page, then each
    // "Load earlier". Reset when the filter or org changes.
    let mut pages = use_signal(Vec::<Activity>::new);
    let mut exhausted = use_signal(|| false);
    let mut loading = use_signal(|| false);
    use_effect(move || {
        let f = filter();
        let Some((slug, _)) = target() else { return };
        pages.set(Vec::new());
        exhausted.set(false);
        loading.set(true);
        spawn(async move {
            match crate::feeds::fetch_activity_page(&slug, f.clone()).await {
                Ok(rows) => {
                    exhausted.set(rows.len() < f.limit.unwrap_or(50) as usize);
                    pages.set(rows);
                }
                Err(e) => tracing::warn!(error = %e, "activity page fetch failed"),
            }
            loading.set(false);
        });
    });

    let picked_projects = project_ids.read().clone();
    let f = filter();
    let rows: Vec<Activity> = merge_newest_first([store.list(), pages.read().clone()])
        .into_iter()
        .filter(|a| f.matches(a))
        .filter(|a| {
            picked_projects.is_empty() || a.project_id.is_some_and(|p| picked_projects.contains(&p))
        })
        .collect();

    let load_earlier = move |_| {
        let Some((slug, _)) = target() else { return };
        let mut f = filter();
        let oldest = merge_newest_first([store.list(), pages.read().clone()])
            .into_iter()
            .rfind(|a| f.matches(a));
        f.before = oldest.as_ref().map(ActivityCursor::after);
        loading.set(true);
        spawn(async move {
            match crate::feeds::fetch_activity_page(&slug, f.clone()).await {
                Ok(rows) => {
                    exhausted.set(rows.len() < f.limit.unwrap_or(50) as usize);
                    pages.write().extend(rows);
                }
                Err(e) => tracing::warn!(error = %e, "activity scroll-back failed"),
            }
            loading.set(false);
        });
    };

    let mark_all_read = move |()| {
        let Some((slug, _)) = target() else { return };
        spawn(async move {
            if let Err(e) = crate::feeds::mark_all_notifications_read(&slug).await {
                tracing::warn!(error = %e, "mark all notifications read failed");
            }
        });
    };

    let today = Local::now().date_naive();
    let now = Utc::now();
    let days = group_by_day(&rows, today);
    let narrowed = groups.read().len() + project_ids.read().len() + usize::from(only_mine());
    let project_rows: Vec<(Uuid, String)> = {
        let mut v: Vec<(Uuid, String)> = projects
            .value()
            .map(|r| {
                r.iter()
                    .filter(|(_, p)| !p.project.archived)
                    .map(|(_, p)| (p.project.id, p.project.title.clone()))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        v.dedup_by_key(|p| p.0);
        v
    };
    let q = query().to_lowercase();

    rsx! {
        div { class: "mx-auto flex w-full max-w-3xl flex-col px-4 pt-5 pb-10 sm:px-8",
            div { class: "-mx-4 -mt-5 pb-1 sm:-mx-8",
                task_ui_core::section_header::SectionHeader {
                    title: "Activity".to_owned(),
                    action: rsx! { architect_ui::lucide_dioxus::CheckCheck { size: 19 } },
                    action_label: "Mark all notifications read".to_owned(),
                    on_action: mark_all_read,
                    badge: narrowed,
                    task_ui_core::section_header::SectionMenuItem {
                        label: "Filter",
                        icon: rsx! { architect_ui::lucide_dioxus::ListFilter { size: 18 } },
                        note: (narrowed > 0).then(|| format!("{narrowed} on")),
                        on_click: move |()| sheet.set(true),
                    }
                    task_ui_core::section_header::SectionMenuItem {
                        label: "Only mine",
                        icon: rsx! { architect_ui::lucide_dioxus::User { size: 18 } },
                        note: Some(if only_mine() { "On" } else { "Off" }.to_owned()),
                        on_click: move |()| only_mine.toggle(),
                    }
                    if narrowed > 0 {
                        task_ui_core::section_header::SectionMenuDivider {}
                        task_ui_core::section_header::SectionMenuItem {
                            label: "Clear filters",
                            icon: rsx! { architect_ui::lucide_dioxus::X { size: 18 } },
                            on_click: move |()| {
                                groups.set(HashSet::new());
                                project_ids.set(HashSet::new());
                                only_mine.set(false);
                            },
                        }
                    }
                }
            }

            // Desk: the filter as chips under the header (the sheet is
            // the phone's).
            div { class: "hidden flex-wrap items-center gap-1.5 pb-3 md:flex",
                for g in ActivityGroup::ALL {
                    {
                        let on = groups.read().contains(&g);
                        rsx! {
                            button {
                                key: "{g.label()}",
                                r#type: "button",
                                aria_pressed: "{on}",
                                class: if on { "h-7 rounded-full bg-foreground/[0.12] px-3 text-[12px] text-foreground" } else { "h-7 rounded-full px-3 text-[12px] text-muted-foreground shadow-[inset_0_0_0_1px_rgb(127_127_127/0.25)] hover:text-foreground" },
                                onclick: move |_| {
                                    let mut set = groups.write();
                                    if !set.remove(&g) {
                                        set.insert(g);
                                    }
                                },
                                "{g.label()}"
                            }
                        }
                    }
                }
                span { class: "mx-1 h-4 w-px bg-border" }
                button {
                    r#type: "button",
                    aria_pressed: "{only_mine()}",
                    class: if only_mine() { "h-7 rounded-full bg-foreground/[0.12] px-3 text-[12px] text-foreground" } else { "h-7 rounded-full px-3 text-[12px] text-muted-foreground shadow-[inset_0_0_0_1px_rgb(127_127_127/0.25)] hover:text-foreground" },
                    onclick: move |_| only_mine.toggle(),
                    "Only mine"
                }
                if !project_rows.is_empty() {
                    select {
                        class: "h-7 rounded-full bg-transparent px-2 text-[12px] text-muted-foreground shadow-[inset_0_0_0_1px_rgb(127_127_127/0.25)]",
                        aria_label: "Project",
                        onchange: move |e| {
                            let v = e.value();
                            let mut set = HashSet::new();
                            if let Ok(id) = v.parse::<Uuid>() {
                                set.insert(id);
                            }
                            project_ids.set(set);
                        },
                        option { value: "", "All projects" }
                        for (id , title) in project_rows.clone() {
                            option {
                                key: "{id}",
                                value: "{id}",
                                selected: picked_projects.contains(&id),
                                "{title}"
                            }
                        }
                    }
                }
            }

            if rows.is_empty() && !loading() {
                p { class: "py-24 text-center text-[15px] text-muted-foreground",
                    if narrowed > 0 {
                        "Nothing matches these filters."
                    } else {
                        "Nothing has happened yet. Changes, comments, captures and timers land here."
                    }
                }
            }

            for (label , day) in days {
                section { key: "{label}", class: "flex flex-col",
                    h2 { class: "sticky top-0 z-10 bg-background/95 py-2 text-[13px] font-medium text-muted-foreground supports-[backdrop-filter]:backdrop-blur",
                        "{label}"
                    }
                    ul { class: "flex flex-col",
                        for a in day {
                            {
                                let href = a.href.clone();
                                let w = describe(&a);
                                let when = rel_time(a.at, now);
                                let exact = a.at.with_timezone(&Local).format("%b %-d, %H:%M").to_string();
                                rsx! {
                                    li { key: "{a.id}",
                                        button {
                                            r#type: "button",
                                            class: "flex w-full items-start gap-3 rounded-xl px-2 py-2.5 text-left hover:bg-accent/40 active:bg-accent/60 disabled:cursor-default",
                                            disabled: href.is_empty(),
                                            onclick: move |_| {
                                                if !href.is_empty() {
                                                    nav.push(href.as_str());
                                                }
                                            },
                                            Avatar { name: a.actor_name.clone() }
                                            span { class: "flex min-w-0 flex-1 flex-col gap-1",
                                                span { class: "text-[14px] leading-5 text-muted-foreground",
                                                    span { class: "font-medium text-foreground", "{a.actor_name}" }
                                                    " {w.verb} "
                                                    if !w.object.is_empty() {
                                                        span { class: "font-medium text-foreground", "{w.object}" }
                                                    }
                                                    if !w.detail.is_empty() {
                                                        " {w.detail}"
                                                    }
                                                }
                                                if a.kind == ActivityKind::Commented && !a.body.is_empty() {
                                                    span { class: "line-clamp-3 rounded-lg bg-foreground/[0.04] px-3 py-2 text-[13px] leading-5 text-foreground/90",
                                                        "{a.body}"
                                                    }
                                                }
                                            }
                                            span { class: "shrink-0 pt-0.5 text-[12px] tabular-nums text-muted-foreground", title: "{exact}",
                                                "{when}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !rows.is_empty() {
                div { class: "flex justify-center pt-4",
                    if exhausted() {
                        span { class: "text-[12px] text-muted-foreground", "That's the beginning." }
                    } else {
                        button {
                            r#type: "button",
                            class: "h-9 rounded-full px-4 text-[13px] text-muted-foreground shadow-[inset_0_0_0_1px_rgb(127_127_127/0.25)] hover:text-foreground disabled:opacity-50",
                            disabled: loading(),
                            onclick: load_earlier,
                            if loading() { "Loading…" } else { "Load earlier" }
                        }
                    }
                }
            }
        }

        // Phone: the filter sheet.
        task_ui_core::sheet::BottomSheet {
            open: sheet(),
            on_close: move |()| sheet.set(false),
            title: String::new(),
            task_ui_core::sheet::SheetSearch {
                value: query(),
                placeholder: "Filter…".to_owned(),
                on_input: move |v| query.set(v),
                on_close: move |()| sheet.set(false),
            }
            task_ui_core::sheet::SheetSection { label: "Kind".to_owned() }
            task_ui_core::sheet::SheetGroup {
                for g in ActivityGroup::ALL.into_iter().filter(|g| q.is_empty() || g.label().to_lowercase().contains(&q)) {
                    task_ui_core::sheet::SheetCheckRow {
                        key: "{g.label()}",
                        label: g.label().to_owned(),
                        on: groups.read().contains(&g),
                        on_toggle: move |on: bool| {
                            let mut set = groups.write();
                            if on {
                                set.insert(g);
                            } else {
                                set.remove(&g);
                            }
                        },
                    }
                }
            }
            if !project_rows.is_empty() {
                task_ui_core::sheet::SheetSection { label: "Project".to_owned() }
                task_ui_core::sheet::SheetGroup {
                    for (id , title) in project_rows.into_iter().filter(|(_, t)| q.is_empty() || t.to_lowercase().contains(&q)) {
                        task_ui_core::sheet::SheetCheckRow {
                            key: "{id}",
                            label: title,
                            on: picked_projects.contains(&id),
                            on_toggle: move |on: bool| {
                                let mut set = project_ids.write();
                                if on {
                                    set.insert(id);
                                } else {
                                    set.remove(&id);
                                }
                            },
                        }
                    }
                }
            }
            task_ui_core::sheet::SheetSection { label: "Display".to_owned() }
            task_ui_core::sheet::SheetGroup {
                task_ui_core::sheet::SheetToggle {
                    label: "Only mine".to_owned(),
                    hint: Some("What you did".to_owned()),
                    on: only_mine(),
                    on_toggle: move |on| only_mine.set(on),
                }
            }
        }
    }
}

// ── one thing's timeline ─────────────────────────────────────────────

/// A timeline row: a recorded change, or a comment from the thing's
/// thread.
#[derive(Clone, PartialEq)]
enum Item {
    Entry(Activity),
    Comment(threads::Message),
}

impl Item {
    fn at(&self) -> DateTime<Utc> {
        match self {
            Self::Entry(a) => a.at,
            Self::Comment(m) => m.posted_at,
        }
    }
}

/// One thing's history, oldest first, with its comments inline and —
/// when `comments` is on — a composer at the bottom ("Leave a
/// comment…", Enter to send) that posts into the thing's thread
/// (opening one on the first comment).
///
/// `project: true` reads the project's feed (`ActivityFilter::project`)
/// — the project's own lines and its tasks'.
// t[impl activity.ui.entity]
#[component]
pub fn EntityActivity(
    slug: String,
    entity_type: String,
    entity_id: Uuid,
    #[props(default = false)] project: bool,
    #[props(default = true)] comments: bool,
) -> Element {
    let store = stores::use_activity_store();
    let messages_store = stores::use_message_store();
    let thread_muts = stores::use_thread_mutations();
    let account = use_context::<crate::auth::AuthCtx>().active;
    let orgs = use_context::<Signal<Vec<OrgMeta>>>();
    let nav = use_navigator();

    let key = use_memo(use_reactive!(|(slug, entity_type, entity_id, project)| {
        (slug, entity_type, entity_id, project)
    }));
    let filter = use_memo(move || {
        let (_, et, id, project) = key();
        if project {
            ActivityFilter::project(id)
        } else {
            ActivityFilter::entity(&et, id)
        }
    });

    // This thing's history, fetched once per key; live lines arrive
    // through the shared store.
    let mut fetched = use_signal(Vec::<Activity>::new);
    // Its comment thread, when it has one.
    let mut thread = use_signal(|| None::<Uuid>);
    let mut thread_messages = use_signal(Vec::<threads::Message>::new);
    use_effect(move || {
        let (slug, et, id, _) = key();
        let f = filter();
        fetched.set(Vec::new());
        thread.set(None);
        thread_messages.set(Vec::new());
        spawn(async move {
            match crate::feeds::fetch_activity_page(&slug, f).await {
                Ok(rows) => fetched.set(rows),
                Err(e) => tracing::warn!(error = %e, "entity activity fetch failed"),
            }
            if let Ok(ths) = crate::feeds::fetch_threads(&slug, &et, id).await
                && let Some(th) = ths.into_iter().last()
            {
                thread.set(Some(th.id));
                if let Ok(ms) = crate::feeds::fetch_thread_messages(&slug, th.id).await {
                    thread_messages.set(ms);
                }
            }
        });
    });

    let f = filter();
    let here = entity_id.to_string();
    let tid = thread();
    let mut items: Vec<Item> = merge_newest_first([store.list(), fetched.read().clone()])
        .into_iter()
        .filter(|a| f.matches(a))
        // This thing's own comments come from its thread (with their
        // optimistic drafts); its tasks' comments stay as lines.
        .filter(|a| !(a.kind == ActivityKind::Commented && a.entity_id == here && tid.is_some()))
        .map(Item::Entry)
        .collect();
    if let Some(tid) = tid {
        let mut seen = HashSet::new();
        let live = messages_store.list();
        for m in thread_messages.read().iter().chain(live.iter()) {
            if m.thread_id == tid && seen.insert((m.body.clone(), m.author_label.clone())) {
                items.push(Item::Comment(m.clone()));
            }
        }
    }
    items.sort_by_key(Item::at);

    let mut draft = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let mut send = move || {
        let body = draft().trim().to_owned();
        if body.is_empty() || sending() {
            return;
        }
        let (slug, et, id, _) = key();
        let org_id = orgs
            .read()
            .iter()
            .find(|o| o.slug == slug)
            .and_then(|o| o.id)
            .unwrap_or_default();
        let (author_id, author_label) = account
            .read()
            .as_ref()
            .map_or((None, String::new()), |a| (Some(a.user_id), a.name.clone()));
        draft.set(String::new());
        sending.set(true);
        spawn(async move {
            let tid = match thread() {
                Some(t) => Some(t),
                None => crate::feeds::create_thread(
                    &slug,
                    threads::CreateThreadRequest {
                        org_id,
                        entity_type: et.clone(),
                        entity_id: id,
                        anchor_path: None,
                        title: "Comments".to_owned(),
                        kind: String::new(),
                        created_by: author_id.unwrap_or_default(),
                        source_kind: "native".to_owned(),
                        source_ref: None,
                        source_url: None,
                    },
                )
                .await
                .map(|th| th.id)
                .map_err(|e| tracing::warn!(error = %e, "open comment thread failed"))
                .ok(),
            };
            if let Some(tid) = tid {
                thread.set(Some(tid));
                thread_muts.post_message(
                    slug,
                    threads::PostMessageRequest {
                        thread_id: tid,
                        org_id,
                        author_id,
                        author_label,
                        body,
                        reply_to: None,
                        source_kind: "native".to_owned(),
                        external_id: None,
                        original_text: None,
                        source_url: None,
                        posted_at: None,
                    },
                );
            }
            sending.set(false);
        });
    };

    let now = Utc::now();
    let me_name = account
        .read()
        .as_ref()
        .map_or_else(|| "You".to_owned(), |a| a.name.clone());

    rsx! {
        div { class: "flex flex-col",
            if items.is_empty() {
                p { class: "pb-2 text-[13px] text-muted-foreground/70",
                    if comments { "No activity yet. Changes and comments will show here." } else { "No activity yet." }
                }
            }
            ol { class: "flex flex-col",
                for item in items {
                    {match item {
                        Item::Entry(a) => {
                            let line = describe_here(&a, &here);
                            let when = rel_time(a.at, now);
                            let href = (a.entity_id != here).then(|| a.href.clone()).filter(|h| !h.is_empty());
                            let comment_body = (a.kind == ActivityKind::Commented).then(|| a.body.clone());
                            rsx! {
                                li { key: "{a.id}", class: "relative flex flex-col gap-1 py-1.5 pl-7",
                                    span { class: "absolute left-[9px] top-0 h-full w-px bg-border/60" }
                                    span { class: "absolute left-0.5 top-2", Avatar { name: a.actor_name.clone(), small: true } }
                                    span { class: "text-[13px] leading-5 text-muted-foreground",
                                        span { class: "font-medium text-foreground", "{a.actor_name}" }
                                        " "
                                        if let Some(h) = href {
                                            button {
                                                r#type: "button",
                                                class: "text-left hover:text-foreground hover:underline",
                                                onclick: move |_| { nav.push(h.as_str()); },
                                                "{line}"
                                            }
                                        } else {
                                            "{line}"
                                        }
                                        span { class: "text-muted-foreground/70", " · {when}" }
                                    }
                                    if let Some(body) = comment_body.filter(|b| !b.is_empty()) {
                                        span { class: "rounded-lg bg-foreground/[0.04] px-3 py-2 text-[13px] leading-5 text-foreground/90 whitespace-pre-wrap", "{body}" }
                                    }
                                }
                            }
                        }
                        Item::Comment(m) => {
                            let when = rel_time(m.posted_at, now);
                            let who = if m.author_label.is_empty() { "Someone".to_owned() } else { m.author_label.clone() };
                            let pending = m.id.is_nil();
                            rsx! {
                                li { key: "{m.id}-{m.posted_at}", class: "relative flex flex-col gap-1.5 py-2 pl-7",
                                    span { class: "absolute left-[9px] top-0 h-full w-px bg-border/60" }
                                    span { class: "absolute left-0.5 top-2.5", Avatar { name: who.clone(), small: true } }
                                    div { class: if pending { "flex flex-col gap-1 rounded-xl border border-border/60 bg-card px-3 py-2.5 opacity-70" } else { "flex flex-col gap-1 rounded-xl border border-border/60 bg-card px-3 py-2.5" },
                                        span { class: "text-[13px] text-muted-foreground",
                                            span { class: "font-medium text-foreground", "{who}" }
                                            span { class: "text-muted-foreground/70", " · {when}" }
                                        }
                                        span { class: "whitespace-pre-wrap text-[14px] leading-6 text-foreground", "{m.body}" }
                                    }
                                }
                            }
                        }
                    }}
                }
            }
            if comments {
                div { class: "mt-3 flex items-start gap-2 pl-0.5",
                    Avatar { name: me_name, small: true }
                    textarea {
                        class: "min-h-[2.75rem] flex-1 resize-none rounded-xl border border-border/60 bg-card px-3 py-2.5 text-[14px] leading-6 text-foreground outline-none placeholder:text-muted-foreground focus:border-border",
                        rows: "1",
                        placeholder: "Leave a comment…",
                        aria_label: "Leave a comment",
                        value: "{draft}",
                        oninput: move |e| draft.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter && !e.modifiers().shift() {
                                e.prevent_default();
                                send();
                            }
                        },
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: ActivityKind) -> Activity {
        Activity {
            id: Uuid::new_v4(),
            at: Utc::now(),
            actor_id: "u1".into(),
            actor_name: "Alice".into(),
            kind,
            entity_type: "task".into(),
            entity_id: "t1".into(),
            entity_title: "Comp lead vocals".into(),
            href: "/tasks/t1".into(),
            project_id: None,
            field: String::new(),
            from: String::new(),
            to: String::new(),
            body: String::new(),
        }
    }

    // t[verify activity.ui.sentence]
    #[test]
    fn a_line_reads_like_a_sentence() {
        let mut a = entry(ActivityKind::StatusChanged);
        a.from = "open".into();
        a.to = "in-progress".into();
        let w = describe(&a);
        assert_eq!(
            (w.verb.as_str(), w.object.as_str(), w.detail.as_str()),
            ("moved", "Comp lead vocals", "to In Progress")
        );
        assert_eq!(
            describe_here(&a, "t1"),
            "changed status from Open to In Progress"
        );
        // On a project's timeline, a task's line keeps its title.
        assert_eq!(
            describe_here(&a, "p1"),
            "moved Comp lead vocals to In Progress"
        );

        let mut took = entry(ActivityKind::Assigned);
        took.to = "Alice".into();
        assert_eq!(describe(&took).verb, "took");
        assert_eq!(describe_here(&took, "t1"), "self-assigned the task");
        took.to = "Sam".into();
        assert_eq!(describe_here(&took, "t1"), "assigned the task to Sam");

        let mut due = entry(ActivityKind::DueChanged);
        due.to = "2026-10-20".into();
        assert_eq!(describe(&due).detail, "due Oct 20");

        let mut mode = entry(ActivityKind::ModeEnded);
        mode.entity_title = "Maintenance hour".into();
        mode.body = "47 min".into();
        let w = describe(&mode);
        assert_eq!(
            (w.verb.as_str(), w.detail.as_str()),
            ("ended", "after 47 min")
        );

        let mut processed = entry(ActivityKind::Processed);
        processed.field = "task".into();
        processed.to = "Call the tuner".into();
        assert_eq!(describe(&processed).detail, "into task “Call the tuner”");
    }

    #[test]
    fn days_are_named_like_linear_names_them() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(today.pred_opt().unwrap(), today), "Yesterday");
        assert_eq!(
            day_label(NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(), today),
            "Mon, Oct 5"
        );
        assert_eq!(
            day_label(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap(), today),
            "Wed, Dec 31, 2025"
        );
    }

    #[test]
    fn merged_pages_are_one_row_per_id_newest_first() {
        let mut a = entry(ActivityKind::Created);
        a.at = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let mut b = entry(ActivityKind::Completed);
        b.at = a.at + chrono::Duration::minutes(5);
        let rows = merge_newest_first([vec![a.clone(), b.clone()], vec![b.clone()]]);
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![b.id, a.id]
        );
        let days = group_by_day(&rows, b.at.with_timezone(&Local).date_naive());
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].1.len(), 2);
    }

    #[test]
    fn values_read_as_words() {
        assert_eq!(value_label("in-progress"), "In Progress");
        assert_eq!(value_label("open"), "Open");
        assert_eq!(date_label("2026-10-20T00:00:00Z"), "Oct 20");
        assert_eq!(initial("sam"), "S");
    }
}
