//! The Planner — Schedule's default view: one day, top to bottom.
//!
//! After Super Productivity's planner, made for a phone: you mostly need
//! to know what today holds, so it shows one day as a single scrolling
//! page — how full it is, then the day's shape in time (time blocks and
//! events in order, the gaps between them as free time, a line where
//! *now* is), then the work the day owes (overdue, due, in progress) and
//! what got done. The week strip above it moves between days.
//!
//! [`plan_day`] is the whole reading of the day — pure, so it is tested
//! without a renderer; [`Planner`] draws it.

use chrono::{NaiveDate, Timelike};
use dioxus::prelude::*;
use task_proto::TaskInfo;
use task_ui::display::TaskDisplay as _;
use uuid::Uuid;
use view_calendar::{ActualSpan, CalendarEvent, ColorTag, TemplateBlock};

use crate::routes::Route;
use crate::stores::{self, OrgTask};

/// Shorter gaps than this are not called free time.
const MIN_FREE: u16 = 15;

/// Something on the day's timeline.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    /// A day-plan block (opens the block editor).
    Block {
        id: String,
        start: u16,
        end: u16,
        label: String,
        assignment: Option<String>,
        color: ColorTag,
    },
    /// A calendar event.
    Event {
        start: u16,
        end: u16,
        title: String,
        color: ColorTag,
    },
    /// Nothing planned between two things.
    Free { start: u16, end: u16 },
    /// The current minute, on today.
    Now(u16),
}

impl Slot {
    fn start(&self) -> u16 {
        match self {
            Self::Block { start, .. } | Self::Event { start, .. } | Self::Free { start, .. } => {
                *start
            }
            Self::Now(m) => *m,
        }
    }
}

/// One day, read for the planner.
#[derive(Clone, Default)]
pub struct DayPlan {
    /// All-day events, by title.
    pub all_day: Vec<String>,
    /// Blocks, events, free gaps and now, in time order.
    pub timeline: Vec<Slot>,
    /// Open work due before the day (shown on today only).
    pub overdue: Vec<OrgTask>,
    /// Open work due or scheduled on the day.
    pub due: Vec<OrgTask>,
    /// Work in progress (today only).
    pub doing: Vec<OrgTask>,
    /// Work finished on the day.
    pub done: Vec<OrgTask>,
    /// Minutes of blocks and events (overlaps counted once).
    pub planned_min: u32,
    /// Minutes tracked on the day.
    pub tracked_min: u32,
    /// Estimates of the open work above, in minutes.
    pub estimate_min: u32,
    /// Free minutes left in the day's span (after now, on today).
    pub free_min: u32,
}

fn minute_of(t: chrono::DateTime<chrono::Local>) -> u16 {
    u16::try_from(t.hour() * 60 + t.minute()).unwrap_or(0)
}

fn is_open(t: &TaskInfo) -> bool {
    !t.is_done() && t.status != "cancelled" && t.status != "canceled"
}

fn date_of(s: Option<&str>) -> Option<NaiveDate> {
    s.and_then(|s| s.get(..10)).and_then(|d| d.parse().ok())
}

/// Read `day` for the planner. `now` places the now line and decides
/// what counts as today.
#[must_use]
pub fn plan_day(
    day: NaiveDate,
    now: chrono::DateTime<chrono::Local>,
    blocks: &[TemplateBlock],
    events: &[CalendarEvent],
    actuals: &[ActualSpan],
    tasks: &[OrgTask],
) -> DayPlan {
    let today = now.date_naive();
    let mut plan = DayPlan::default();
    let mut timed: Vec<Slot> = Vec::new();
    for b in blocks
        .iter()
        .filter(|b| b.date == day && b.end_min > b.start_min)
    {
        timed.push(Slot::Block {
            id: b.id.clone(),
            start: b.start_min,
            end: b.end_min,
            label: b.label.clone(),
            assignment: b.assignment.clone().filter(|a| !a.is_empty()),
            color: b.color,
        });
    }
    for e in events {
        let (s, f) = (
            e.start.with_timezone(&chrono::Local),
            e.end.with_timezone(&chrono::Local),
        );
        if e.all_day {
            if s.date_naive() <= day && day <= f.date_naive() {
                plan.all_day.push(e.title.clone());
            }
            continue;
        }
        if s.date_naive() != day {
            continue;
        }
        // An event running past midnight ends with the day.
        let end = if f.date_naive() == day {
            minute_of(f)
        } else {
            24 * 60
        };
        timed.push(Slot::Event {
            start: minute_of(s),
            end: end.max(minute_of(s)),
            title: e.title.clone(),
            color: e.color,
        });
    }
    timed.sort_by_key(|s| match s {
        Slot::Block { start, end, .. } | Slot::Event { start, end, .. } => (*start, *end),
        _ => (0, 0),
    });

    // Free time: the gaps in the union of what is planned.
    let mut spans: Vec<(u16, u16)> = timed
        .iter()
        .filter_map(|s| match s {
            Slot::Block { start, end, .. } | Slot::Event { start, end, .. } => Some((*start, *end)),
            _ => None,
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(u16, u16)> = Vec::new();
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    plan.planned_min = merged.iter().map(|(s, e)| u32::from(e - s)).sum();
    let now_min = (day == today).then(|| minute_of(now));
    for w in merged.windows(2) {
        let (start, end) = (w[0].1, w[1].0);
        if end.saturating_sub(start) >= MIN_FREE {
            timed.push(Slot::Free { start, end });
            let from = now_min.map_or(start, |n| start.max(n));
            plan.free_min += u32::from(end.saturating_sub(from));
        }
    }
    if let Some(n) = now_min {
        timed.push(Slot::Now(n));
    }
    // Time order; at one minute, now first, then free, then the rest.
    timed.sort_by_key(|s| {
        let rank = match s {
            Slot::Now(_) => 0,
            Slot::Free { .. } => 1,
            _ => 2,
        };
        (s.start(), rank)
    });
    plan.timeline = timed;

    plan.tracked_min = actuals
        .iter()
        .filter(|a| a.date == day)
        .map(|a| u32::from(a.end_min.saturating_sub(a.start_min)))
        .sum();

    for row in tasks {
        let t = &row.task;
        if !is_open(t) {
            if t.completed_date == Some(day) {
                plan.done.push(row.clone());
            }
            continue;
        }
        let due = t.due_date();
        let scheduled = date_of(t.scheduled.as_deref());
        if due == Some(day) || scheduled == Some(day) {
            plan.due.push(row.clone());
        } else if day == today && due.is_some_and(|d| d < today) {
            plan.overdue.push(row.clone());
        } else if day == today && t.status == "in-progress" {
            plan.doing.push(row.clone());
        }
    }
    for list in [&mut plan.overdue, &mut plan.due, &mut plan.doing] {
        list.sort_by(|a, b| {
            a.task
                .due_date()
                .cmp(&b.task.due_date())
                .then_with(|| a.task.title.cmp(&b.task.title))
        });
    }
    plan.estimate_min = plan
        .overdue
        .iter()
        .chain(&plan.due)
        .chain(&plan.doing)
        .filter_map(|r| r.task.time_estimate)
        .sum();
    plan
}

/// "1h 20m", "45m", "2h".
#[must_use]
pub fn dur(min: u32) -> String {
    match (min / 60, min % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// "9:30" on a 24-hour day.
fn clock(min: u16) -> String {
    format!("{}:{:02}", min / 60, min % 60)
}

/// The bar beside a slot — static class names, so the stylesheet has them.
fn bar(c: ColorTag) -> &'static str {
    match c {
        ColorTag::Neutral => "bg-foreground/30",
        ColorTag::Primary => "bg-primary",
        ColorTag::Success => "bg-emerald-400",
        ColorTag::Warning => "bg-amber-400",
        ColorTag::Danger => "bg-rose-400",
        ColorTag::Info => "bg-sky-400",
    }
}

/// One day of the schedule, top to bottom. `on_block` opens a block's
/// editor.
#[component]
pub fn Planner(
    day: NaiveDate,
    blocks: Vec<TemplateBlock>,
    events: Vec<CalendarEvent>,
    actuals: Vec<ActualSpan>,
    on_block: EventHandler<(NaiveDate, String)>,
) -> Element {
    let tasks = stores::use_task_list();
    let task_muts = stores::use_task_mutations();
    let projects = stores::use_project_store();
    let _projects = stores::use_project_list();
    let nav = use_navigator();
    // Re-read as time passes, so the now line and free time keep up.
    let tick = use_signal(|| 0u64);
    crate::chrome::use_second_tick(tick);
    let _ = tick();

    let rows: Vec<OrgTask> = tasks
        .value()
        .map(|r| stores::unique_tasks(r).into_iter().cloned().collect())
        .unwrap_or_default();
    let now = chrono::Local::now();
    let plan = plan_day(day, now, &blocks, &events, &actuals, &rows);
    let project_titles: std::collections::HashMap<Uuid, String> = projects
        .list()
        .iter()
        .map(|r| (r.project.id, r.project.title.clone()))
        .collect();
    let is_today = day == now.date_naive();
    let open = plan.overdue.len() + plan.due.len() + plan.doing.len();
    let over = plan.estimate_min > plan.free_min && plan.free_min > 0;
    let load = if plan.free_min == 0 {
        0.0
    } else {
        (f64::from(plan.estimate_min) / f64::from(plan.free_min)).min(1.0) * 100.0
    };

    let task_row = move |row: OrgTask, done: bool| {
        let t = row.task.clone();
        let id = t.id;
        let project = t.project_id.and_then(|p| project_titles.get(&p).cloned());
        let estimate = t.time_estimate.filter(|m| *m > 0).map(dur);
        let contexts = t.contexts.0.join(" ");
        let late = !done && t.due_date().is_some_and(|d| d < now.date_naive());
        let slug = row.slug.clone();
        rsx! {
            div { key: "{id}", class: "flex min-h-12 items-center gap-3 rounded-xl px-2 py-1.5 active:bg-accent/50",
                button {
                    r#type: "button",
                    "data-testid": "planner-check",
                    class: if done { "flex size-6 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground" } else { "flex size-6 shrink-0 items-center justify-center rounded-full border-2 border-foreground/30 active:bg-foreground/10" },
                    "aria-label": if done { "Mark open" } else { "Mark done" },
                    onclick: move |e| {
                        e.stop_propagation();
                        let mut next = t.clone();
                        if done {
                            next.status = "todo".to_owned();
                            next.completed_date = None;
                        } else {
                            next.status = "done".to_owned();
                            next.completed_date = Some(chrono::Local::now().date_naive());
                        }
                        task_muts.apply(&slug, task_ui::TaskMutation::Update { task: next });
                    },
                    if done { architect_ui::lucide_dioxus::Check { size: 14 } }
                }
                button {
                    r#type: "button",
                    class: "flex min-w-0 flex-1 flex-col text-left",
                    onclick: move |_| {
                        nav.push(Route::TaskDetailRoute { id });
                    },
                    span { class: if done { "truncate text-[16px] text-muted-foreground line-through" } else { "truncate text-[16px] text-foreground" },
                        "{row.task.title}"
                    }
                    if project.is_some() || !contexts.is_empty() || late {
                        span { class: "flex items-center gap-1.5 truncate text-[13px] text-muted-foreground",
                            if late {
                                span { class: "text-rose-400", "Late" }
                            }
                            if let Some(p) = project { span { class: "truncate", "{p}" } }
                            if !contexts.is_empty() { span { class: "truncate", "{contexts}" } }
                        }
                    }
                }
                if let Some(e) = estimate {
                    span { class: "shrink-0 text-[13px] tabular-nums text-muted-foreground", "{e}" }
                }
            }
        }
    };
    let section = |title: &'static str, n: usize| {
        rsx! {
            div { class: "flex items-center gap-2 px-2 pb-1 pt-5",
                span { class: "text-[12px] font-semibold uppercase tracking-[0.06em] text-muted-foreground/80", "{title}" }
                span { class: "text-[12px] tabular-nums text-muted-foreground/60", "{n}" }
            }
        }
    };

    rsx! {
        div {
            "data-testid": "planner",
            class: "mx-auto flex w-full max-w-2xl flex-col px-3 pb-32 pt-2 sm:px-6",
            // ── How full the day is ──
            div { class: "rounded-2xl bg-card px-4 py-3.5",
                div { class: "flex items-baseline gap-5",
                    div { class: "flex flex-col",
                        span { class: "text-[20px] font-semibold tabular-nums text-foreground", "{dur(plan.planned_min)}" }
                        span { class: "text-[12px] text-muted-foreground", "planned" }
                    }
                    div { class: "flex flex-col",
                        span { class: "text-[20px] font-semibold tabular-nums text-foreground", "{dur(plan.tracked_min)}" }
                        span { class: "text-[12px] text-muted-foreground", "tracked" }
                    }
                    div { class: "flex flex-col",
                        span { class: "text-[20px] font-semibold tabular-nums text-foreground", "{open}" }
                        span { class: "text-[12px] text-muted-foreground", if open == 1 { "task" } else { "tasks" } }
                    }
                }
                if plan.estimate_min > 0 && plan.free_min > 0 {
                    div { class: "mt-3 flex flex-col gap-1.5",
                        div { class: "h-1.5 overflow-hidden rounded-full bg-foreground/[0.08]",
                            div {
                                class: if over { "h-full rounded-full bg-rose-400" } else { "h-full rounded-full bg-primary" },
                                style: "width: {load:.0}%",
                            }
                        }
                        span { class: if over { "text-[13px] text-rose-300" } else { "text-[13px] text-muted-foreground" },
                            if over {
                                "{dur(plan.estimate_min)} of work, {dur(plan.free_min)} free — over by {dur(plan.estimate_min - plan.free_min)}"
                            } else {
                                "{dur(plan.estimate_min)} of work fits in {dur(plan.free_min)} free"
                            }
                        }
                    }
                }
            }
            // ── All day ──
            if !plan.all_day.is_empty() {
                div { class: "flex flex-wrap gap-1.5 px-1 pt-3",
                    for (i, t) in plan.all_day.iter().enumerate() {
                        span { key: "{i}", class: "rounded-full bg-foreground/[0.07] px-3 py-1 text-[13px] text-foreground/85", "{t}" }
                    }
                }
            }
            // ── The day's shape ──
            div { class: "flex flex-col pt-3",
                if plan.timeline.iter().all(|s| matches!(s, Slot::Now(_))) {
                    p { class: "px-2 py-6 text-center text-[15px] text-muted-foreground",
                        "Nothing planned. Tap + to add a block."
                    }
                }
                for (i, slot) in plan.timeline.iter().cloned().enumerate() {
                    match slot {
                        Slot::Now(m) => rsx! {
                            div { key: "s{i}", "data-testid": "planner-now", class: "flex items-center gap-2 py-1",
                                span { class: "w-12 shrink-0 text-right text-[12px] font-semibold tabular-nums text-rose-400", "{clock(m)}" }
                                span { class: "size-2 shrink-0 rounded-full bg-rose-400" }
                                span { class: "h-px flex-1 bg-rose-400/60" }
                            }
                        },
                        Slot::Free { start, end } => {
                            let past = is_today && end <= now_minute(now);
                            rsx! {
                                div { key: "s{i}", class: if past { "flex items-center gap-3 py-1 opacity-40" } else { "flex items-center gap-3 py-1" },
                                    span { class: "w-12 shrink-0" }
                                    span { class: "w-1 shrink-0 self-stretch rounded-full border-l-2 border-dashed border-foreground/15" }
                                    span { class: "text-[13px] text-muted-foreground/70", "{dur(u32::from(end - start))} free" }
                                }
                            }
                        }
                        Slot::Block { id, start, end, label, assignment, color } => {
                            let past = is_today && end <= now_minute(now);
                            let live = is_today && (start..end).contains(&now_minute(now));
                            rsx! {
                                button {
                                    key: "s{i}",
                                    r#type: "button",
                                    "data-testid": "planner-block",
                                    class: if past { "flex w-full items-stretch gap-3 rounded-xl py-2 text-left opacity-50 active:bg-accent/50" } else if live { "flex w-full items-stretch gap-3 rounded-xl bg-accent/50 py-2 text-left active:bg-accent" } else { "flex w-full items-stretch gap-3 rounded-xl py-2 text-left active:bg-accent/50" },
                                    onclick: move |_| on_block.call((day, id.clone())),
                                    span { class: "flex w-12 shrink-0 flex-col items-end pt-0.5",
                                        span { class: "text-[13px] font-medium tabular-nums text-foreground/85", "{clock(start)}" }
                                        span { class: "text-[12px] tabular-nums text-muted-foreground/70", "{clock(end)}" }
                                    }
                                    span { class: "w-1 shrink-0 rounded-full {bar(color)}" }
                                    span { class: "flex min-w-0 flex-1 flex-col",
                                        span { class: "truncate text-[16px] text-foreground", "{label}" }
                                        if let Some(a) = assignment {
                                            span { class: "truncate text-[13px] text-muted-foreground", "{a}" }
                                        }
                                    }
                                    span { class: "shrink-0 pr-2 pt-0.5 text-[13px] tabular-nums text-muted-foreground", "{dur(u32::from(end - start))}" }
                                }
                            }
                        }
                        Slot::Event { start, end, title, color } => {
                            let past = is_today && end <= now_minute(now);
                            rsx! {
                                div {
                                    key: "s{i}",
                                    "data-testid": "planner-event",
                                    class: if past { "flex items-stretch gap-3 py-2 opacity-50" } else { "flex items-stretch gap-3 py-2" },
                                    span { class: "flex w-12 shrink-0 flex-col items-end pt-0.5",
                                        span { class: "text-[13px] font-medium tabular-nums text-foreground/85", "{clock(start)}" }
                                        span { class: "text-[12px] tabular-nums text-muted-foreground/70", "{clock(end)}" }
                                    }
                                    span { class: "w-1 shrink-0 rounded-full {bar(color)}" }
                                    span { class: "flex min-w-0 flex-1 items-center gap-1.5",
                                        span { class: "text-muted-foreground", architect_ui::lucide_dioxus::CalendarDays { size: 14 } }
                                        span { class: "truncate text-[16px] text-foreground", "{title}" }
                                    }
                                    span { class: "shrink-0 pr-2 pt-0.5 text-[13px] tabular-nums text-muted-foreground", "{dur(u32::from(end - start))}" }
                                }
                            }
                        }
                    }
                }
            }
            // ── What the day owes ──
            if !plan.overdue.is_empty() {
                {section("Overdue", plan.overdue.len())}
                for r in plan.overdue.iter().cloned() { {task_row(r, false)} }
            }
            if !plan.doing.is_empty() {
                {section("In progress", plan.doing.len())}
                for r in plan.doing.iter().cloned() { {task_row(r, false)} }
            }
            if !plan.due.is_empty() {
                {section(if is_today { "Due today" } else { "Due" }, plan.due.len())}
                for r in plan.due.iter().cloned() { {task_row(r, false)} }
            }
            if !plan.done.is_empty() {
                {section("Done", plan.done.len())}
                for r in plan.done.iter().cloned() { {task_row(r, true)} }
            }
        }
    }
}

fn now_minute(now: chrono::DateTime<chrono::Local>) -> u16 {
    minute_of(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    fn at(h: u32, m: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 10, 15, h, m, 0)
            .single()
            .unwrap()
    }

    fn block(id: &str, s: u16, e: u16) -> TemplateBlock {
        TemplateBlock {
            id: id.into(),
            date: day(),
            label: id.into(),
            start_min: s,
            end_min: e,
            color: ColorTag::Primary,
            assignment: None,
            soft: false,
            into_next: false,
            from_prev: false,
        }
    }

    fn task(title: &str, due: Option<&str>, status: &str, est: Option<u32>) -> OrgTask {
        let mut t = TaskInfo::new(title);
        t.id = Uuid::new_v4();
        t.due = due.map(str::to_owned);
        t.status = status.into();
        t.time_estimate = est;
        OrgTask {
            slug: "acme-audio".into(),
            task: t,
        }
    }

    #[test]
    fn the_day_reads_in_time_order_with_free_time_and_now() {
        let blocks = [
            block("Deep work", 9 * 60, 11 * 60),
            block("Mix", 13 * 60, 15 * 60),
        ];
        let p = plan_day(day(), at(12, 0), &blocks, &[], &[], &[]);
        let starts: Vec<u16> = p.timeline.iter().map(Slot::start).collect();
        assert_eq!(starts, vec![9 * 60, 11 * 60, 12 * 60, 13 * 60]);
        assert!(matches!(
            p.timeline[1],
            Slot::Free {
                start: 660,
                end: 780
            }
        ));
        assert!(matches!(p.timeline[2], Slot::Now(720)));
        assert_eq!(p.planned_min, 240);
        assert_eq!(p.free_min, 60, "free time left counts from now");
    }

    #[test]
    fn another_day_has_no_now_line_and_short_gaps_are_not_free() {
        let blocks = [
            block("A", 9 * 60, 10 * 60),
            block("B", 10 * 60 + 10, 11 * 60),
        ];
        let p = plan_day(
            day(),
            at(12, 0) + chrono::Duration::days(1),
            &blocks,
            &[],
            &[],
            &[],
        );
        assert!(
            !p.timeline
                .iter()
                .any(|s| matches!(s, Slot::Now(_) | Slot::Free { .. }))
        );
    }

    #[test]
    fn the_work_the_day_owes_and_whether_it_fits() {
        let tasks = [
            task("Bounce stems", Some("2026-10-15"), "todo", Some(90)),
            task("Invoice", Some("2026-10-12"), "todo", Some(30)),
            task("Comp vocals", None, "in-progress", None),
            task("Next week", Some("2026-10-22"), "todo", Some(60)),
        ];
        let blocks = [block("A", 9 * 60, 10 * 60), block("B", 11 * 60, 12 * 60)];
        let p = plan_day(day(), at(8, 0), &blocks, &[], &[], &tasks);
        assert_eq!(p.due.len(), 1);
        assert_eq!(p.overdue.len(), 1);
        assert_eq!(p.doing.len(), 1);
        assert_eq!(p.estimate_min, 120);
        assert_eq!(p.free_min, 60, "one free hour between the blocks");
        // Another day owes only what is due on it.
        let later = plan_day(
            day() + chrono::Duration::days(7),
            at(8, 0),
            &[],
            &[],
            &[],
            &tasks,
        );
        assert_eq!(later.due.len(), 1);
        assert!(later.overdue.is_empty() && later.doing.is_empty());
    }

    #[test]
    fn durations_read_short() {
        assert_eq!(dur(45), "45m");
        assert_eq!(dur(120), "2h");
        assert_eq!(dur(80), "1h 20m");
    }
}
