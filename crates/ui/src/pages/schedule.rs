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
    // The day and view in sight — the page's, so the phone header and
    // week strip drive the calendar.
    let mut cal_anchor = use_signal(|| chrono::Local::now().date_naive());
    // The view is remembered per device; with none chosen yet, a phone
    // opens on Day (seven columns don't fit) and a desk on Week.
    let stored_view = crate::local_store::get(VIEW_KEY).and_then(|v| view_from_key(&v));
    let mut cal_view = use_signal(|| stored_view.unwrap_or(ViewMode::Week));
    let mut new_event = use_signal(|| 0u64);
    use_hook(move || {
        if stored_view.is_none() {
            spawn(async move {
                let mut probe = dioxus::document::eval(
                    "dioxus.send(window.matchMedia('(max-width: 640px)').matches)",
                );
                if let Ok(true) = probe.recv::<bool>().await {
                    cal_view.set(ViewMode::Day);
                }
            });
        }
    });
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
    // Which apps may claim a file the overlay links to.
    let plugins = crate::nav::use_active_plugins();

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
            b.category,
        ))
    });
    // The mode the open block can start: it is today's, running now, and
    // its kind steers the hour.
    let focus_mode = crate::now_block::use_focus_mode();
    let timers = stores::use_timer_mutations();
    let active_timer = crate::chrome::use_active_timer();
    let sheet_mode = editor.as_ref().and_then(|(date, _, _, s, e, _, cat)| {
        use chrono::Timelike;
        let now = chrono::Local::now();
        let minute = u16::try_from(now.hour() * 60 + now.minute()).ok()?;
        let label = cat.focus_label()?;
        if *date != now.date_naive() || minute < *s || minute >= *e {
            return None;
        }
        let name = task_proto::suggest::mode_name(*cat).to_owned();
        Some(
            match focus_mode.read().as_ref().filter(|m| m.category == *cat) {
                Some(m) => crate::pages::block_sheet::ModeAction::Running(
                    name,
                    crate::chrome::fmt_hms((chrono::Utc::now() - m.started).num_seconds().max(0)),
                ),
                None => {
                    let _ = label;
                    crate::pages::block_sheet::ModeAction::Start(name)
                }
            },
        )
    });
    let sheet_mode_cat = editor.as_ref().map(|t| t.6);
    let toggle_mode = move || {
        if let Some(m) = focus_mode.peek().clone() {
            timers.stop(m.slug, m.user_id, m.session_id);
            return;
        }
        let Some(label) = sheet_mode_cat.and_then(|c| c.focus_label()) else {
            return;
        };
        let Some((slug, org_id)) = crate::chrome::resolve_org(&selection.read(), &org_list.read())
        else {
            return;
        };
        let req = timer_proto::StartTimerRequest {
            user_id: crate::chrome::owner_id(org_id),
            org_id,
            project_id: None,
            project_path: String::new(),
            task_note_path: String::new(),
            description: label.to_owned(),
        };
        match active_timer.as_ref() {
            Some(at) => timers.switch(slug, at.session.id, req),
            None => timers.start(slug, req),
        }
    };
    let (tasks, projects) = pickers().unwrap_or_default();

    // The named store mutations behind each edit surface — optimistic
    // store patch + write-through, rollback + Notifications on failure.
    let mut save_block = move |(date, id): (NaiveDate, String),
                               label: String,
                               s: u16,
                               e: u16,
                               assign: Option<Assign>,
                               category: BlockCategory| {
        let Some(slug) = slug() else { return };
        let label = if label.is_empty() {
            "Block".to_owned()
        } else {
            label
        };
        plan_muts.save_block(
            slug,
            date,
            id,
            label,
            (s, e),
            assign.map(to_assignment),
            category,
        );
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

    // ── The phone header ──
    let today = chrono::Local::now().date_naive();
    let header_title = match cal_view() {
        ViewMode::Day => view_calendar::day_title(cal_anchor(), today),
        ViewMode::Month => cal_anchor().format("%B").to_string(),
        // The week's own day header (full width, under the title) says
        // which days; the title says the month — or both, across one.
        ViewMode::Week | ViewMode::Blocks => {
            let a = cal_anchor();
            let monday = a - chrono::Duration::days(i64::from(a.weekday().num_days_from_monday()));
            let sunday = monday + chrono::Duration::days(6);
            if monday.month() == sunday.month() {
                monday.format("%B").to_string()
            } else {
                format!("{} – {}", monday.format("%b"), sunday.format("%b"))
            }
        }
    };
    // A new block on the day in view: from the next hour today, else
    // 09:00 — then straight into its sheet to name it.
    let mut add_block_now = move || {
        let Some(slug) = slug() else { return };
        let date = cal_anchor();
        let start = if date == chrono::Local::now().date_naive() {
            use chrono::Timelike;
            let h = chrono::Local::now().hour() + 1;
            u16::try_from(h.min(23) * 60).unwrap_or(540)
        } else {
            540
        };
        let end = (start + 60).min(1440);
        let id = plan_muts.add_block(
            slug,
            date,
            "New block".to_owned(),
            BlockCategory::Allocatable,
            (start, end),
        );
        editing.set(Some((date, id)));
    };
    // Days with an event, for the week strip's dots (every day has
    // plan blocks, so those would dot everything).
    let busy_days: Vec<NaiveDate> = state
        .read()
        .events
        .values()
        .map(|e| e.start.with_timezone(&chrono::Local).date_naive())
        .collect();

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
        // Phone: exactly the screen under the status bar — the grid
        // scrolls under the floating tab bar (the shell's bottom padding
        // is given back), so the document itself never scrolls.
        div { class: "flex h-[calc(100dvh-env(safe-area-inset-top,0px))] -mb-[calc(6rem+env(safe-area-inset-bottom,0px))] flex-col overflow-hidden md:mb-0 md:h-[calc(100vh-3.5rem)] lg:h-screen",
            match &*templates.read_unchecked() {
                Some(Err(e)) => rsx! {
                    div { class: "mx-3 mt-2 shrink-0",
                        crate::states::InlineError {
                            message: e.clone(),
                            label: "Day-plan templates".to_string(),
                        }
                    }
                },
                // Having no templates is a normal state, not a fault: a
                // hint on a desk, nothing on a phone, where three lines of
                // file paths sat above the calendar on every visit.
                Some(Ok(t)) if t.is_empty() => rsx! {
                    div { class: "mx-3 mt-2 hidden shrink-0 rounded-md border border-border/60 bg-muted/30 px-3 py-2 text-xs text-muted-foreground md:block",
                        "Showing events only. Plan your days by adding weekday.md and weekend.md under Projects/Scheduling/templates."
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
            // Phone: the day as the title, + for a new block, ⋯ for the
            // views and the day's plan; the week under it in reach.
            div { class: "shrink-0 md:hidden",
                task_ui_core::section_header::SectionHeader {
                    title: header_title,
                    // The week leads the header (tap a day, swipe for the
                    // next week); the month view keeps its name.
                    lead: (cal_view() == ViewMode::Day).then(|| rsx! {
                        view_calendar::WeekStrip {
                            anchor: cal_anchor(),
                            on_pick: move |d: NaiveDate| {
                                cal_anchor.set(d);
                            },
                            busy: busy_days.clone(),
                            compact: true,
                        }
                    }),
                    action: rsx! { architect_ui::lucide_dioxus::Plus { size: 21 } },
                    action_label: "New block".to_owned(),
                    on_action: move |()| add_block_now(),
                    if cal_anchor() != today {
                        task_ui_core::section_header::SectionMenuItem {
                            label: "Go to today",
                            icon: rsx! { architect_ui::lucide_dioxus::CalendarCheck { size: 18 } },
                            on_click: move |()| cal_anchor.set(today),
                        }
                    }
                    for (mode, name) in [
                        (ViewMode::Day, "Day"),
                        (ViewMode::Week, "Week"),
                        (ViewMode::Month, "Month"),
                        (ViewMode::Blocks, "Blocks"),
                    ] {
                        task_ui_core::section_header::SectionMenuItem {
                            key: "{name}",
                            label: name,
                            icon: rsx! { architect_ui::lucide_dioxus::CalendarDays { size: 18 } },
                            note: (cal_view() == mode).then(|| "✓".to_owned()),
                            on_click: move |()| {
                                cal_view.set(mode);
                                crate::local_store::set(VIEW_KEY, view_key(mode));
                            },
                        }
                    }
                    task_ui_core::section_header::SectionMenuDivider {}
                    task_ui_core::section_header::SectionMenuItem {
                        label: "New event",
                        icon: rsx! { architect_ui::lucide_dioxus::CalendarPlus { size: 18 } },
                        on_click: move |()| new_event += 1,
                    }
                    task_ui_core::section_header::SectionMenuItem {
                        label: "Reset day to template",
                        icon: rsx! { architect_ui::lucide_dioxus::RotateCcw { size: 18 } },
                        on_click: move |()| reset_day(cal_anchor()),
                    }
                }
            }
            // The calendar takes what is left under the header — sized to
            // the whole page it overflowed, and its focus scrolled the
            // header away.
            div { class: "min-h-0 flex-1",
            Calendar {
                events,
                template_blocks,
                anchor: Some(cal_anchor),
                view: Some(cal_view),
                phone_chrome: true,
                new_event: Some(new_event),
                summary,
                slot_rows,
                slot_items,
                // A meal in the overlay is a recipe file. Which app
                // opens it is the apps' to say — this page knows only
                // that a slot item is a file somebody may want to read.
                on_slot_item: move |id: String| {
                    if let Some((app, target)) =
                        task_plugin_ui::claim_file(&id, |p| plugins.contains(p))
                    {
                        nav.push(crate::routes::plugin_route(app, &target));
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
        }
        if let Some((date, id, label, start_min, end_min, assignment, category)) = editor {
            crate::pages::block_sheet::BlockSheet {
                key: "{date}-{id}",
                label,
                start_min,
                end_min,
                category,
                assignment,
                tasks,
                projects,
                mode: sheet_mode,
                on_mode: move |()| toggle_mode(),
                on_save: {
                    let id = id.clone();
                    move |(l, s, e, a, c): crate::pages::block_sheet::BlockSave| {
                        save_block((date, id.clone()), l, s, e, a, c);
                    }
                },
                on_delete: move |()| {
                    if let Some(slug) = slug() {
                        plan_muts.remove_block(slug, date, id.clone());
                    }
                    editing.set(None);
                },
                on_close: move |()| editing.set(None),
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

/// Where this device remembers the Schedule's view.
const VIEW_KEY: &str = "task.schedule.view";

fn view_key(v: ViewMode) -> &'static str {
    match v {
        ViewMode::Day => "day",
        ViewMode::Week => "week",
        ViewMode::Month => "month",
        ViewMode::Blocks => "blocks",
    }
}

fn view_from_key(s: &str) -> Option<ViewMode> {
    match s {
        "day" => Some(ViewMode::Day),
        "week" => Some(ViewMode::Week),
        "month" => Some(ViewMode::Month),
        "blocks" => Some(ViewMode::Blocks),
        _ => None,
    }
}
