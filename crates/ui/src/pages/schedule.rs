//! `/schedule` — calendar with the editable per-day plan.
//!
//! Each visible date gets a resolved plan row — the saved
//! `scheduling_proto::DayPlan` if the user has edited that date,
//! otherwise materialized from the matching `weekday` / `weekend`
//! template — loaded through the shared day-plan store
//! ([`stores::use_dayplan_list`]). The plan's blocks render on the
//! calendar as clickable guides; clicking one opens an editor to
//! move / relabel it, and every edit goes through the named
//! [`stores::DayPlanMutations`] (optimistic write + reconcile-or-
//! rollback, failures in the Notifications tray).
//!
//! Real calendar events load from + persist to the CalendarEvents
//! service; blocks support drag-to-move/resize on the grid. Assigning
//! a task or project into a block happens in the block editor (click
//! a block → pick from the task/project lists).

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate, Utc};
use dioxus::prelude::*;
use architect_ui::prelude::*;
use scheduling_proto::{BlockAssignment, BlockCategory, CalEvent};
use view_calendar::{
    BlockEdit, Calendar, CalendarEvent, CalendarMutation, CalendarState, ColorTag, EventId,
    TemplateBlock, ViewMode, apply,
};

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores::{self, DayPlanRow};

/// An assignment as the editor passes it back: `(kind, title, ref_id)`
/// — `kind` is `"label"` / `"task"` / `"project"`.
type Assign = (String, String, Option<String>);

/// `(id, title)` options for the assignment pickers.
type PickList = Vec<(String, String)>;

#[component]
pub fn ScheduleView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we read/write plans for (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let templates = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_day_templates(&s).await,
            None => Ok(Vec::new()),
        }
    });

    // Tasks + projects to assign into blocks (the pickers).
    let pickers = use_resource(move || async move {
        let Some(s) = slug() else {
            return (PickList::new(), PickList::new());
        };
        let slugs = [s];
        let tasks = crate::feeds::fetch_tasks_tagged(&slugs)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(_, t)| (t.id.to_string(), t.title))
            .collect::<PickList>();
        let projects = crate::feeds::fetch_projects(&slugs)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.id.to_string(), p.title))
            .collect::<PickList>();
        (tasks, projects)
    });

    // Visible date range + the per-date plans we've loaded/materialized
    // for it. Seeded to the current week so blocks materialize on first
    // paint; the calendar's `on_range` then keeps it in step as the user
    // navigates (don't wait on that callback for the initial render).
    let mut range = use_signal(|| {
        let today = chrono::Local::now().date_naive();
        let monday =
            today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()));
        Some((monday, monday + chrono::Duration::days(6)))
    });
    // The templates as the day-plan store hook wants them: `None`
    // while the fetch is still resolving (keeps the list `Loading`).
    let tpl_list = use_memo(move || match &*templates.read() {
        Some(Ok(t)) => Some(t.clone()),
        _ => None,
    });
    // Every visible date's resolved plan (saved blocks + soft template
    // fallback) as one shared-store list; all writes go through
    // [`stores::DayPlanMutations`] (optimistic, rollback-on-failure).
    let plans_result = stores::use_dayplan_list(range(), tpl_list());
    let plan_muts = stores::use_dayplan_mutations();
    // Which (date, block_id) is being edited, if any.
    let mut editing = use_signal(|| None::<(NaiveDate, String)>);
    // Planned meals — previewed inside Meal-category blocks
    // ("what's for dinner on that date").
    let meals = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_meal_plans(&s).await.unwrap_or_default(),
            None => Vec::new(),
        }
    });
    // Real events — loaded from + persisted to the CalendarEvents
    // service.
    let mut state = use_signal(CalendarState::default);
    let loaded_events = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::list_events(&s).await,
            None => Ok(Vec::new()),
        }
    });
    use_effect(move || {
        if let Some(Ok(evs)) = &*loaded_events.read() {
            let mut st = CalendarState::default();
            for e in evs {
                if let Some(ce) = from_proto(e) {
                    st.events.insert(ce.id, ce);
                }
            }
            state.set(st);
        }
    });

    // Apply a mutation locally, then persist the affected event.
    let mut on_event = move |mu: CalendarMutation| {
        apply(&mut state.write(), &mu);
        let Some(slug) = slug() else { return };
        match &mu {
            CalendarMutation::Remove { id } => {
                let id = id.to_string();
                spawn(async move {
                    let _ = crate::feeds::delete_event(&slug, &id).await;
                });
            }
            _ => {
                if let Some(id) = affected_id(&mu) {
                    if let Some(ev) = state.peek().events.get(&id).cloned() {
                        let ce = to_proto(&ev);
                        spawn(async move {
                            let _ = crate::feeds::upsert_event(&slug, ce).await;
                        });
                    }
                }
            }
        }
    };

    let events = state.read().events.values().cloned().collect::<Vec<_>>();
    let meal_lookup = build_meal_lookup(meals().as_deref().unwrap_or(&[]));
    // Stale-while-revalidate: navigating the range keeps the last rows
    // rendered while the refetch is in flight.
    let plan_rows: Vec<DayPlanRow> = plans_result
        .value()
        .map(|rows| rows.iter().map(|(_, r)| r.clone()).collect())
        .unwrap_or_default();
    let plans_err = plans_result.error().cloned();
    let template_blocks = build_blocks(&plan_rows, &meal_lookup);
    let nav = use_navigator();

    // The same meals, on the block axis. The time grid can only show a
    // meal where a day-plan block happens to sit; this shows every
    // planned meal, including the ones with no time attached at all.
    let (slot_rows, slot_items) = build_meal_slots(meals().as_deref().unwrap_or(&[]));

    // The block currently under edit, resolved to its values.
    let editor = editing().and_then(|(date, id)| {
        let row = plan_rows.iter().find(|r| r.date == date)?;
        let b = row.plan.blocks.iter().find(|b| b.id.0 == id)?;
        let assignment = b
            .assignment
            .as_ref()
            .map(|a| (a.kind.clone(), a.title.clone(), a.ref_id.clone()));
        Some((
            date,
            id,
            b.label.clone(),
            b.start.minutes_since_midnight,
            b.end.minutes_since_midnight,
            assignment,
        ))
    });
    let (tasks, projects) = pickers().unwrap_or_default();

    // The named store mutations behind each edit surface — optimistic
    // store patch + write-through, rollback + Notifications on failure.
    let mut save_block = move |(date, id): (NaiveDate, String),
                               label: String,
                               s: u16,
                               e: u16,
                               assign: Option<Assign>| {
        let Some(slug) = slug() else { return };
        plan_muts.save_block(slug, date, id, label, (s, e), assign.map(to_assignment));
        editing.set(None);
    };

    // Move/retime a block from a grid drag, possibly across days, then
    // persist the affected day plan(s).
    let move_block = move |orig: NaiveDate, target: NaiveDate, id: String, s: u16, e: u16| {
        let Some(slug) = slug() else { return };
        plan_muts.move_block(slug, orig, target, id, (s, e));
    };

    // Revert a date to its template — drop the saved plan, re-materialize.
    let mut reset_day = move |date: NaiveDate| {
        let Some(slug) = slug() else { return };
        plan_muts.reset_day(slug, date, &tpl_list().unwrap_or_default());
        editing.set(None);
    };

    // Allocatable-block usage across the visible range.
    let overview = {
        let r = range();
        let (mut alloc_min, mut blocks, mut assigned) = (0i64, 0u32, 0u32);
        for row in &plan_rows {
            if !r.is_some_and(|(s, e)| row.date >= s && row.date <= e) {
                continue;
            }
            for b in row.plan.blocks.iter() {
                if matches!(b.category, BlockCategory::Allocatable) {
                    blocks += 1;
                    alloc_min += i64::from(b.end.minutes_since_midnight)
                        - i64::from(b.start.minutes_since_midnight);
                    if b.assignment.is_some() {
                        assigned += 1;
                    }
                }
            }
        }
        (alloc_min.max(0) as f64 / 60.0, blocks, assigned)
    };

    // One-line usage context, folded into the calendar toolbar on
    // wide screens instead of costing the page its own row.
    let summary = (overview.1 > 0).then(|| {
        format!(
            "{:.1}h allocatable · {}/{} assigned",
            overview.0, overview.2, overview.1
        )
    });

    rsx! {
        // The calendar owns the single toolbar row; the page just
        // hands it the full viewport and surfaces load problems as
        // compact banners above it. Phones subtract the mobile chrome
        // with `dvh` (browser-UI collapse can't hide the grid bottom);
        // `md:`+ keeps the desktop top-bar math.
        div { class: "flex h-[calc(100dvh-8rem)] flex-col overflow-hidden pb-14 md:h-[calc(100vh-3.5rem)] md:pb-0 lg:h-screen",
            match &*templates.read_unchecked() {
                Some(Err(e)) => rsx! {
                    div { class: "mx-3 mt-2 shrink-0",
                        crate::states::InlineError {
                            message: e.clone(),
                            label: "Day-plan templates".to_string(),
                        }
                    }
                },
                Some(Ok(t)) if t.is_empty() => rsx! {
                    div { class: "mx-3 mt-2 shrink-0 rounded-md border border-border/60 bg-muted/30 px-3 py-2 text-xs text-muted-foreground",
                        "No day-plan templates for this org under Projects/Scheduling/templates/ (weekday.md / weekend.md) — showing events only."
                    }
                },
                _ => rsx! {},
            }
            if let Some(e) = plans_err {
                div { class: "mx-3 mt-2 shrink-0",
                    crate::states::InlineError {
                        message: e,
                        label: "Day plans".to_string(),
                    }
                }
            }
            Calendar {
                events,
                template_blocks,
                initial_view: Some(ViewMode::Week),
                summary,
                slot_rows,
                slot_items,
                on_slot_item: move |id: String| {
                    if id.ends_with(".cook") {
                        nav.push(crate::routes::Route::RecipeReadRoute { path: id });
                    }
                },
                on_range: move |(s, e)| range.set(Some((s, e))),
                on_block_click: move |(date, id)| editing.set(Some((date, id))),
                on_block_edit: move |(orig, target, id, s, e): BlockEdit| {
                    move_block(orig, target, id, s, e);
                },
                on_event: move |mu| on_event(mu),
            }
        }
        if let Some((date, id, label, start_min, end_min, assignment)) = editor {
            BlockEditor {
                key: "{date}-{id}",
                label,
                start_min,
                end_min,
                assignment,
                tasks,
                projects,
                on_save: move |(l, s, e, a): (String, u16, u16, Option<Assign>)| {
                    save_block((date, id.clone()), l, s, e, a);
                },
                on_reset: move |()| reset_day(date),
                on_cancel: move |()| editing.set(None),
            }
        }
    }
}

/// Modal to move / relabel / assign a plan block. Holds its own working
/// values so typing doesn't churn the page; commits on Save.
#[component]
fn BlockEditor(
    label: ReadSignal<String>,
    start_min: u16,
    end_min: u16,
    assignment: ReadSignal<Option<Assign>>,
    tasks: ReadSignal<PickList>,
    projects: ReadSignal<PickList>,
    on_save: EventHandler<(String, u16, u16, Option<Assign>)>,
    on_reset: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    // Working copies seeded once per mount (the parent keys this
    // modal per block) — peek, so upstream writes don't churn the
    // user's draft mid-edit.
    let mut lbl = use_signal(move || label.peek().clone());
    let mut start = use_signal(|| start_min);
    let mut end = use_signal(|| end_min);
    let mut assign = use_signal(move || assignment.peek().clone());
    // The task/project picker binds a String; on_change dispatches into `assign`.
    let pick_sel = use_signal(String::new);

    let assign_title = assign().map(|a| a.1).unwrap_or_default();
    // Copy signal handles for the picker's change handler — no list
    // clones; the `for` loops below read the same signals.
    let pick_tasks = tasks;
    let pick_projects = projects;
    let tasks = tasks.read();
    let projects = projects.read();

    let input_cls = "rounded-md border border-border bg-background px-2 py-1.5 text-sm text-foreground outline-none focus:ring-2 focus:ring-primary/40";

    rsx! {
        // Centered modal on desktop; below `sm` it docks to the
        // bottom edge (bottom-sheet convention), capped + scrollable,
        // safe-area padded.
        div {
            // Centered dialog on desktop; bottom sheet on phones
            // (matches the app's other mobile editors); capped +
            // scrollable so long forms never overflow the viewport.
            class: "fixed inset-0 z-50 flex items-end justify-center bg-black/40 sm:items-center sm:p-4",
            onclick: move |_| on_cancel.call(()),
            div {
                class: "flex max-h-[85dvh] w-full flex-col gap-3 overflow-y-auto rounded-t-2xl border border-border bg-card p-5 shadow-xl sm:max-w-sm sm:rounded-xl",
                style: "padding-bottom: max(1.25rem, env(safe-area-inset-bottom, 0px));",
                onclick: move |e| e.stop_propagation(),
                Heading { level: HeadingLevel::H3, "Edit block" }
                label { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                    "Label"
                    input {
                        class: "{input_cls}",
                        value: "{lbl}",
                        oninput: move |e| lbl.set(e.value()),
                    }
                }
                div { class: "flex gap-3",
                    label { class: "flex flex-1 flex-col gap-1 text-xs text-muted-foreground",
                        "Start"
                        input {
                            class: "{input_cls}",
                            r#type: "time",
                            value: "{fmt_minute_of_day(start())}",
                            oninput: move |e| {
                                if let Some(m) = parse_time(&e.value()) {
                                    start.set(m);
                                }
                            },
                        }
                    }
                    label { class: "flex flex-1 flex-col gap-1 text-xs text-muted-foreground",
                        "End"
                        input {
                            class: "{input_cls}",
                            r#type: "time",
                            value: "{fmt_minute_of_day(end())}",
                            oninput: move |e| {
                                if let Some(m) = parse_time(&e.value()) {
                                    end.set(m);
                                }
                            },
                        }
                    }
                }
                // Assignment — a free label, or pick a task / project.
                div { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                    "Assignment"
                    input {
                        class: "{input_cls}",
                        placeholder: "Type a label, or pick below",
                        value: "{assign_title}",
                        oninput: move |e| {
                            let v = e.value();
                            assign.set(if v.trim().is_empty() {
                                None
                            } else {
                                Some(("label".into(), v, None))
                            });
                        },
                    }
                    Select {
                        value: pick_sel,
                        placeholder: "— pick task / project —".to_string(),
                        on_change: move |v: String| {
                            if let Some(id) = v.strip_prefix("task:") {
                                if let Some((_, t)) = pick_tasks.read().iter().find(|(i, _)| i == id) {
                                    assign.set(Some(("task".into(), t.clone(), Some(id.to_string()))));
                                }
                            } else if let Some(id) = v.strip_prefix("project:") {
                                if let Some((_, t)) = pick_projects.read().iter().find(|(i, _)| i == id) {
                                    assign.set(Some(("project".into(), t.clone(), Some(id.to_string()))));
                                }
                            } else if v == "__clear" {
                                assign.set(None);
                            }
                        },
                        SelectContent {
                            SelectItem { value: "__clear".to_string(), index: 0, "— clear —" }
                            if !tasks.is_empty() {
                                SelectGroup {
                                    SelectLabel { "Tasks" }
                                    for (i, (id, title)) in tasks.iter().enumerate() {
                                        SelectItem { key: "t-{id}", value: "task:{id}", index: i + 1, "{title}" }
                                    }
                                }
                            }
                            if !projects.is_empty() {
                                SelectGroup {
                                    SelectLabel { "Projects" }
                                    for (i, (id, title)) in projects.iter().enumerate() {
                                        SelectItem { key: "p-{id}", value: "project:{id}", index: i + 1 + tasks.len(), "{title}" }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "mt-1 flex flex-wrap items-center justify-between gap-2",
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Small,
                        on_click: move |_| on_reset.call(()),
                        "Reset day to template"
                    }
                    div { class: "flex gap-2",
                        Button {
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Small,
                            on_click: move |_| on_cancel.call(()),
                            "Cancel"
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Small,
                            on_click: move |_| on_save.call((lbl(), start(), end(), assign())),
                            "Save"
                        }
                    }
                }
            }
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────────

/// The editor's `(kind, title, ref_id)` tuple as the proto assignment.
fn to_assignment((kind, title, ref_id): Assign) -> BlockAssignment {
    BlockAssignment {
        kind,
        title,
        ref_id,
    }
}

/// `(date, slot)` → planned meal titles for the schedule's meal
/// preview. Only `planned`/`cooked` meals; skipped ones don't show.
/// Planned meals as block-axis rows and items.
///
/// Breakfast / lunch / dinner always, then any other slot in use. Kept
/// beside [`build_meal_lookup`] rather than sharing it: that one exists
/// to annotate a day-plan block that already has a time, and drops
/// skipped meals because a time block for something you didn't eat is
/// noise. The block axis has no such constraint — "we ate out on
/// Tuesday" is exactly the sort of thing a week view should say.
fn build_meal_slots(
    meals: &[mealplan_proto::Meal],
) -> (Vec<view_calendar::SlotRow>, Vec<view_calendar::SlotItem>) {
    use mealplan_proto::{Slot, Status};
    use std::collections::BTreeSet;
    use view_calendar::{ColorTag, SlotItem, SlotRow};

    let core = ["breakfast", "lunch", "dinner"];
    let key = |m: &mealplan_proto::Meal| {
        Slot::from_str(&m.slot)
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| m.slot.trim().to_lowercase())
    };

    let mut extra: BTreeSet<String> = BTreeSet::new();
    for m in meals {
        let k = key(m);
        if !core.contains(&k.as_str()) {
            extra.insert(k);
        }
    }
    let rows = core
        .iter()
        .map(|k| (*k).to_string())
        .chain(extra)
        .map(|k| {
            let mut c = k.chars();
            let label = match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => k.clone(),
            };
            SlotRow::new(k, label)
        })
        .collect();

    let items = meals
        .iter()
        .map(|m| {
            let (color, muted) = match Status::from_str(&m.status) {
                Some(Status::Cooked) => (ColorTag::Success, false),
                Some(Status::Skipped) => (ColorTag::Neutral, true),
                Some(Status::EatingOut) => (ColorTag::Warning, false),
                _ => (ColorTag::Primary, false),
            };
            let id = m
                .recipe_paths
                .first()
                .cloned()
                .unwrap_or_else(|| m.id.to_string());
            SlotItem::new(id, m.scheduled_for, key(m), m.name.clone())
                .color(color)
                .muted(muted)
        })
        .collect();

    (rows, items)
}

fn build_meal_lookup(meals: &[mealplan_proto::Meal]) -> HashMap<(NaiveDate, String), Vec<String>> {
    let mut out: HashMap<(NaiveDate, String), Vec<String>> = HashMap::new();
    for m in meals {
        if matches!(m.status.as_str(), "skipped" | "eating-out") {
            continue;
        }
        let slot = mealplan_proto::Slot::from_str(&m.slot)
            .map_or_else(|| m.slot.to_ascii_lowercase(), |s| s.as_str().to_string());
        out.entry((m.scheduled_for, slot))
            .or_default()
            .push(m.name.clone());
    }
    out
}

/// Convert the loaded plans into dated calendar overlay blocks,
/// splitting any block that wraps past midnight. Soft (template-
/// fallback) blocks carry the `soft` flag for dashed/faded
/// rendering; Meal blocks with nothing assigned preview the meal
/// planned for that date + slot.
fn build_blocks(
    rows: &[DayPlanRow],
    meals: &HashMap<(NaiveDate, String), Vec<String>>,
) -> Vec<TemplateBlock> {
    let mut out = Vec::new();
    for row in rows {
        let date = row.date;
        for b in row.plan.blocks.iter() {
            let start = b.start.minutes_since_midnight;
            let end = b.end.minutes_since_midnight;
            let color = category_color(b.category);
            let soft = row.soft_ids.contains(&b.id.0);
            let assignment = b.assignment.as_ref().map(|a| a.title.clone()).or_else(|| {
                if b.category != BlockCategory::Meal {
                    return None;
                }
                let slot = scheduling_proto::resolve::meal_slot_for_block(&b.label, b.start);
                meals
                    .get(&(date, slot.to_string()))
                    .map(|names| names.join(" · "))
            });
            let mk = |start_min, end_min| TemplateBlock {
                id: b.id.0.clone(),
                date,
                label: b.label.clone(),
                start_min,
                end_min,
                color,
                assignment: assignment.clone(),
                soft,
            };
            if end <= start {
                if start < 1440 {
                    out.push(mk(start, 1440));
                }
                if end > 0 {
                    out.push(mk(0, end));
                }
            } else {
                out.push(mk(start, end));
            }
        }
    }
    out
}

/// Which event a mutation touches (`None` for removal — handled
/// separately).
fn affected_id(mu: &CalendarMutation) -> Option<EventId> {
    match mu {
        CalendarMutation::Create { event } => Some(event.id),
        CalendarMutation::Reschedule { id, .. }
        | CalendarMutation::Rename { id, .. }
        | CalendarMutation::Recolor { id, .. }
        | CalendarMutation::SetAllDay { id, .. }
        | CalendarMutation::SetDescription { id, .. }
        | CalendarMutation::SetRecurrence { id, .. } => Some(*id),
        CalendarMutation::Remove { .. } => None,
    }
}

fn to_proto(e: &CalendarEvent) -> CalEvent {
    CalEvent {
        id: e.id.to_string(),
        title: e.title.clone(),
        start: e.start.to_rfc3339(),
        end: e.end.to_rfc3339(),
        all_day: e.all_day,
        color: color_name(e.color).to_string(),
        description: e.description.clone(),
        recurrence: e.recurrence.clone(),
    }
}

fn from_proto(e: &CalEvent) -> Option<CalendarEvent> {
    Some(CalendarEvent {
        id: uuid::Uuid::parse_str(&e.id).ok()?,
        title: e.title.clone(),
        start: chrono::DateTime::parse_from_rfc3339(&e.start)
            .ok()?
            .with_timezone(&Utc),
        end: chrono::DateTime::parse_from_rfc3339(&e.end)
            .ok()?
            .with_timezone(&Utc),
        all_day: e.all_day,
        color: color_from_name(&e.color),
        description: e.description.clone(),
        recurrence: e.recurrence.clone(),
    })
}

fn color_name(c: ColorTag) -> &'static str {
    match c {
        ColorTag::Neutral => "neutral",
        ColorTag::Primary => "primary",
        ColorTag::Success => "success",
        ColorTag::Warning => "warning",
        ColorTag::Danger => "danger",
        ColorTag::Info => "info",
    }
}

fn color_from_name(s: &str) -> ColorTag {
    match s {
        "neutral" => ColorTag::Neutral,
        "success" => ColorTag::Success,
        "warning" => ColorTag::Warning,
        "danger" => ColorTag::Danger,
        "info" => ColorTag::Info,
        _ => ColorTag::Primary,
    }
}

fn category_color(c: BlockCategory) -> ColorTag {
    match c {
        BlockCategory::Allocatable => ColorTag::Success,
        BlockCategory::Reset | BlockCategory::Maintenance => ColorTag::Info,
        BlockCategory::Spiritual | BlockCategory::WindDown => ColorTag::Primary,
        BlockCategory::Meal => ColorTag::Warning,
        BlockCategory::Exercise => ColorTag::Danger,
        BlockCategory::Hygiene | BlockCategory::Sleep | BlockCategory::Other => ColorTag::Neutral,
    }
}

fn fmt_minute_of_day(min: u16) -> String {
    let m = min.min(1439);
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn parse_time(s: &str) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    if h < 24 && m < 60 {
        Some(h * 60 + m)
    } else {
        None
    }
}
