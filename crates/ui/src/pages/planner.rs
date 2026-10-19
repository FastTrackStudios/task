//! The Planner — Schedule's default view: one day, top to bottom.
//!
//! After Super Productivity's planner, made for a phone: you mostly need
//! to know what today holds, so it shows one day as a single scrolling
//! page — its shape in time: time blocks and events in order, the gaps
//! between them as free time, a line where *now* is. Meal blocks open the
//! meal sheet. The work itself is the task list's, not repeated here
//! ([`plan_day`] still reads it, for a later day view). The week strip
//! above it moves between days.
//!
//! [`plan_day`] is the whole reading of the day — pure, so it is tested
//! without a renderer; [`Planner`] draws it.

use chrono::{NaiveDate, Timelike};
use dioxus::prelude::*;
use task_proto::TaskInfo;
use task_ui::display::TaskDisplay as _;
use view_calendar::{ActualSpan, CalendarEvent, ColorTag, TemplateBlock};

use crate::stores::OrgTask;

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
        /// A Meal-kind block: it opens the meal sheet, not the editor.
        meal: bool,
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
    /// The current minute, on today. When it falls inside a block or an
    /// event, that slot draws the now line through itself and the
    /// timeline holds no separate [`Slot::Now`].
    pub now: Option<u16>,
}

impl DayPlan {
    /// Where the now line crosses a slot from `start` to `end`, as a
    /// fraction of its height — `None` unless now is inside it.
    #[must_use]
    pub fn now_within(&self, start: u16, end: u16) -> Option<f32> {
        let n = self.now?;
        (start..end)
            .contains(&n)
            .then(|| f32::from(n - start) / f32::from(end - start))
    }
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
    meals: &[String],
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
            meal: meals.contains(&b.id),
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
    plan.now = now_min;
    // Between things, now is its own line; inside one, that slot shows it.
    if let Some(n) = now_min {
        if !merged.iter().any(|(s, e)| (*s..*e).contains(&n)) {
            timed.push(Slot::Now(n));
        }
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

/// "9 AM", "12:30 PM" — a 12-hour clock; midnight at either end of the
/// day is 12 AM.
#[must_use]
pub fn clock(min: u16) -> String {
    let (h, m) = ((min / 60) % 24, min % 60);
    let half = if h < 12 { "AM" } else { "PM" };
    let h12 = match h % 12 {
        0 => 12,
        h => h,
    };
    if m == 0 {
        format!("{h12} {half}")
    } else {
        format!("{h12}:{m:02} {half}")
    }
}

/// The now line drawn across a live block or event, `frac` of the way
/// down it: the time in the gutter, a dot on the bar, the rule over the
/// rest. Sits over the slot without taking taps.
#[component]
fn NowAcross(min: u16, frac: f32) -> Element {
    // The live slot is drawn taller with its start and end times at its
    // top and bottom; the line keeps to the middle half so it never
    // covers them, still reading earlier-higher, later-lower.
    let pct = 25.0 + frac.clamp(0.0, 1.0) * 50.0;
    rsx! {
        div {
            "data-testid": "planner-now",
            class: "pointer-events-none absolute inset-x-0 z-10 flex -translate-y-1/2 items-center gap-2",
            style: "top: {pct}%",
            span { class: "w-16 shrink-0 rounded bg-background/90 pr-1 text-right text-[12px] font-semibold tabular-nums text-rose-400", "{clock(min)}" }
            span { class: "-ml-1 size-2 shrink-0 rounded-full bg-rose-400" }
            span { class: "h-px flex-1 bg-rose-400/70" }
        }
    }
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
    /// Ids of the blocks that are meals (breakfast, lunch, a snack…).
    #[props(default)]
    meal_blocks: Vec<String>,
    on_block: EventHandler<(NaiveDate, String)>,
    /// A meal block was tapped: `(day, its label)`.
    on_meal: EventHandler<(NaiveDate, String)>,
) -> Element {
    // Re-read as time passes, so the now line and free time keep up.
    let tick = use_signal(|| 0u64);
    crate::chrome::use_second_tick(tick);
    let _ = tick();
    let now = chrono::Local::now();
    // The day's shape only — the work itself is the task list's.
    let plan = plan_day(day, now, &blocks, &events, &actuals, &[], &meal_blocks);
    let is_today = day == now.date_naive();

    rsx! {
        div {
            "data-testid": "planner",
            class: "mx-auto flex w-full max-w-2xl flex-col px-3 pb-32 pt-1 sm:px-6",
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
                                span { class: "w-16 shrink-0 text-right text-[12px] font-semibold tabular-nums text-rose-400", "{clock(m)}" }
                                span { class: "size-2 shrink-0 rounded-full bg-rose-400" }
                                span { class: "h-px flex-1 bg-rose-400/60" }
                            }
                        },
                        Slot::Free { start, end } => {
                            let past = is_today && end <= now_minute(now);
                            rsx! {
                                div { key: "s{i}", class: if past { "flex items-center gap-3 py-1 opacity-40" } else { "flex items-center gap-3 py-1" },
                                    span { class: "w-16 shrink-0" }
                                    span { class: "w-1 shrink-0 self-stretch rounded-full border-l-2 border-dashed border-foreground/15" }
                                    span { class: "text-[13px] text-muted-foreground/70", "{dur(u32::from(end - start))} free" }
                                }
                            }
                        }
                        Slot::Block { id, start, end, label, assignment, color, meal } => {
                            let tap_label = label.clone();
                            let past = is_today && end <= now_minute(now);
                            let live = is_today && (start..end).contains(&now_minute(now));
                            let across = plan.now_within(start, end);
                            rsx! {
                                button {
                                    key: "s{i}",
                                    r#type: "button",
                                    "data-testid": "planner-block",
                                    class: if past { "relative flex w-full items-stretch gap-3 rounded-xl py-2 text-left opacity-50 active:bg-accent/50" } else if live { "relative flex min-h-28 w-full items-stretch gap-3 rounded-xl bg-accent/50 py-2 text-left active:bg-accent" } else { "relative flex w-full items-stretch gap-3 rounded-xl py-2 text-left active:bg-accent/50" },
                                    onclick: move |_| {
                                        if meal {
                                            on_meal.call((day, tap_label.clone()));
                                        } else {
                                            on_block.call((day, id.clone()));
                                        }
                                    },
                                    if let (Some(frac), Some(n)) = (across, plan.now) {
                                        NowAcross { min: n, frac }
                                    }
                                    span { class: if across.is_some() { "flex w-16 shrink-0 flex-col items-end justify-between py-0.5" } else { "flex w-16 shrink-0 flex-col items-end pt-0.5" },
                                        span { class: "text-[13px] font-medium tabular-nums text-foreground/85", "{clock(start)}" }
                                        span { class: "text-[12px] tabular-nums text-muted-foreground/70", "{clock(end)}" }
                                    }
                                    span { class: "w-1 shrink-0 rounded-full {bar(color)}" }
                                    span { class: "flex min-w-0 flex-1 flex-col",
                                        span { class: "flex items-center gap-1.5 truncate text-[16px] text-foreground",
                                            if meal {
                                                span { class: "text-amber-400", architect_ui::lucide_dioxus::Utensils { size: 14 } }
                                            }
                                            "{label}"
                                        }
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
                            let across = plan.now_within(start, end);
                            rsx! {
                                div {
                                    key: "s{i}",
                                    "data-testid": "planner-event",
                                    class: if past { "relative flex items-stretch gap-3 py-2 opacity-50" } else if across.is_some() { "relative flex min-h-28 items-stretch gap-3 py-2" } else { "relative flex items-stretch gap-3 py-2" },
                                    if let (Some(frac), Some(n)) = (across, plan.now) {
                                        NowAcross { min: n, frac }
                                    }
                                    span { class: if across.is_some() { "flex w-16 shrink-0 flex-col items-end justify-between py-0.5" } else { "flex w-16 shrink-0 flex-col items-end pt-0.5" },
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
    use uuid::Uuid;

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
        let p = plan_day(day(), at(12, 0), &blocks, &[], &[], &[], &[]);
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
        let p = plan_day(day(), at(8, 0), &blocks, &[], &[], &tasks, &[]);
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
            &[],
        );
        assert_eq!(later.due.len(), 1);
        assert!(later.overdue.is_empty() && later.doing.is_empty());
    }

    #[test]
    fn a_meal_block_is_marked_so_it_opens_the_meal_sheet() {
        let blocks = [
            block("lunch", 12 * 60 + 30, 13 * 60 + 30),
            block("studio", 9 * 60, 12 * 60),
        ];
        let p = plan_day(
            day(),
            at(8, 0),
            &blocks,
            &[],
            &[],
            &[],
            &["lunch".to_owned()],
        );
        let meal = |id: &str| {
            p.timeline
                .iter()
                .any(|s| matches!(s, Slot::Block { id: i, meal: true, .. } if i == id))
        };
        assert!(meal("lunch"));
        assert!(!meal("studio"));
    }

    #[test]
    fn now_inside_a_block_is_drawn_through_it_not_beside_it() {
        let blocks = [
            block("studio", 9 * 60, 12 * 60),
            block("lunch", 12 * 60, 13 * 60),
        ];
        let p = plan_day(day(), at(12, 15), &blocks, &[], &[], &[], &[]);
        assert!(
            !p.timeline.iter().any(|s| matches!(s, Slot::Now(_))),
            "no separate now line while lunch is on"
        );
        assert_eq!(p.now, Some(12 * 60 + 15));
        assert_eq!(p.now_within(12 * 60, 13 * 60), Some(0.25));
        assert_eq!(p.now_within(9 * 60, 12 * 60), None, "studio has ended");
    }

    #[test]
    fn the_clock_reads_twelve_hour() {
        assert_eq!(clock(0), "12 AM");
        assert_eq!(clock(9 * 60 + 30), "9:30 AM");
        assert_eq!(clock(12 * 60), "12 PM");
        assert_eq!(clock(13 * 60 + 5), "1:05 PM");
        assert_eq!(clock(24 * 60), "12 AM");
    }

    #[test]
    fn durations_read_short() {
        assert_eq!(dur(45), "45m");
        assert_eq!(dur(120), "2h");
        assert_eq!(dur(80), "1h 20m");
    }
}
