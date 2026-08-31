//! The week's food, at a glance.
//!
//! A meal has a date and a slot, and that is all the structure it has —
//! "Friday dinner", not "Friday 18:30–19:15". So this drives
//! [`view_calendar::SlotGrid`], the categorical sibling of the calendar's
//! time grid: seven day columns crossed with breakfast / lunch / dinner,
//! plus whatever other slot the week actually uses.
//!
//! Everything here is a *mapping*. The grid lives in the calendar crate
//! and knows nothing about food, which is what lets `/schedule` show the
//! same view later without either surface owning the other's geometry.

use architect_ui::lucide_dioxus::{ChevronLeft, ChevronRight, UtensilsCrossed};
use architect_ui::prelude::*;
use chrono::{Local, NaiveDate};
use dioxus::prelude::*;
use mealplan_proto::{Meal, Slot, Status};
use std::collections::BTreeSet;
use view_calendar::{ColorTag, SlotGrid, SlotItem, SlotRow};

/// The slots always shown, in the order a day runs. Anything else the
/// week uses — a snack, a supper — is appended after these, so the
/// common shape stays stable while the grid still tells the whole truth.
const CORE_SLOTS: [Slot; 3] = [Slot::Breakfast, Slot::Lunch, Slot::Dinner];

/// A meal's slot as a canonical key, falling back to whatever the vault
/// holds so an unrecognised slot still gets a row rather than vanishing.
fn slot_key(meal: &Meal) -> String {
    Slot::from_str(&meal.slot)
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| meal.slot.trim().to_lowercase())
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s.to_string(),
    }
}

/// How a meal's status reads on the grid. Cooked is settled, skipped
/// and eaten-out are deliberate non-events worth keeping visible, and
/// anything still planned is the default.
fn tone(status: Option<Status>) -> (ColorTag, bool) {
    match status {
        Some(Status::Cooked) => (ColorTag::Success, false),
        Some(Status::Skipped) => (ColorTag::Neutral, true),
        Some(Status::EatingOut) => (ColorTag::Warning, false),
        _ => (ColorTag::Primary, false),
    }
}

#[component]
pub fn MealWeek(slug: Memo<Option<String>>) -> Element {
    let nav = use_navigator();
    let today = use_hook(|| Local::now().date_naive());
    let mut anchor = use_signal(move || today);

    let meals = use_resource(move || async move {
        let s = slug()?;
        crate::fetch_meal_plans(&s).await.ok()
    });

    let days: Vec<NaiveDate> = view_calendar::time::week_days(anchor()).to_vec();
    let all = meals.read().clone().flatten().unwrap_or_default();
    let week: Vec<&Meal> = all
        .iter()
        .filter(|m| days.contains(&m.scheduled_for))
        .collect();

    // Rows: the three that always show, then any extra the week uses.
    let mut extra: BTreeSet<String> = BTreeSet::new();
    let core: Vec<String> = CORE_SLOTS.iter().map(|s| s.as_str().to_string()).collect();
    for m in &week {
        let k = slot_key(m);
        if !core.contains(&k) {
            extra.insert(k);
        }
    }
    let rows: Vec<SlotRow> = core
        .iter()
        .cloned()
        .chain(extra)
        .map(|k| {
            let label = title_case(&k);
            SlotRow::new(k, label)
        })
        .collect();

    // Meals → grid items. The recipe path rides along as the item id so
    // a click can land where you'd actually cook from.
    let items: Vec<SlotItem> = week
        .iter()
        .map(|m| {
            let (color, muted) = tone(Status::from_str(&m.status));
            let id = m
                .recipe_paths
                .first()
                .cloned()
                .unwrap_or_else(|| m.id.to_string());
            let mut item = SlotItem::new(id, m.scheduled_for, slot_key(m), m.name.clone())
                .color(color)
                .muted(muted);
            if m.servings > 1 {
                item = item.detail(format!("{} servings", m.servings));
            }
            item
        })
        .collect();

    let planned = week.len();
    let span = format!(
        "{} – {}",
        days[0].format("%-d %b"),
        days[6].format("%-d %b")
    );

    // An empty week is ambiguous: nothing planned, or nothing planned
    // *here*? Those want different responses, and only one of them is
    // the view's fault. So when this week is bare but meals exist
    // elsewhere, point at the closest one rather than leaving a dead
    // grid and seven paging clicks between the reader and their food.
    let nearest = (planned == 0)
        .then(|| {
            all.iter()
                .map(|m| m.scheduled_for)
                .min_by_key(|d| (*d - days[0]).num_days().abs())
        })
        .flatten();

    rsx! {
        section { class: "flex flex-col gap-3",
            div { class: "flex flex-wrap items-center justify-between gap-3",
                div { class: "flex items-baseline gap-3",
                    Heading { level: HeadingLevel::H2, "This week" }
                    span { class: "text-sm text-muted-foreground", "{span}" }
                }
                div { class: "flex items-center gap-1",
                    span { class: "mr-2 text-xs tabular-nums text-muted-foreground", "{planned} planned" }
                    button {
                        class: "flex size-8 items-center justify-center rounded-md border border-border text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
                        aria_label: "Previous week",
                        onclick: move |_| anchor.set(anchor() - chrono::Duration::days(7)),
                        ChevronLeft { size: 15 }
                    }
                    button {
                        class: "rounded-md border border-border px-2.5 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
                        onclick: move |_| anchor.set(today),
                        "Today"
                    }
                    button {
                        class: "flex size-8 items-center justify-center rounded-md border border-border text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
                        aria_label: "Next week",
                        onclick: move |_| anchor.set(anchor() + chrono::Duration::days(7)),
                        ChevronRight { size: 15 }
                    }
                }
            }

            SlotGrid {
                days: days.clone(),
                rows,
                items,
                today: days.contains(&today).then_some(today),
                on_item: move |id: String| {
                    // Only a recipe path is worth navigating to; a meal
                    // with no recipe falls back to its uuid, which isn't
                    // a route.
                    if id.ends_with(".cook") {
                        nav.push(task_plugin_ui::href_param(crate::APP_ID, "recipe/read", "path", &id));
                    }
                },
            }

            if planned == 0 {
                div { class: "flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border border-dashed border-border px-4 py-6",
                    UtensilsCrossed { size: 15 }
                    match nearest {
                        Some(d) => rsx! {
                            Text { variant: TextVariant::Muted, class: "text-sm",
                                "Nothing planned this week. The closest week with meals is {d.format(\"%-d %b\")}."
                            }
                            button {
                                class: "rounded-md border border-border px-2.5 py-1.5 text-xs text-foreground transition-colors hover:bg-muted",
                                onclick: move |_| anchor.set(d),
                                "Go to that week"
                            }
                        },
                        None => rsx! {
                            Text { variant: TextVariant::Muted, class: "text-sm",
                                "Nothing planned this week yet."
                            }
                        },
                    }
                }
            }
        }
    }
}
