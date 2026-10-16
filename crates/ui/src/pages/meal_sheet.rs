//! The meal sheet — what a food block on the Planner opens.
//!
//! A meal block (breakfast, lunch, dinner, a snack — any block of the
//! Meal kind) is where you plan what to eat and record what you ate:
//! the meals planned for that slot, a recipe from the cookbook to plan,
//! and a line to log what was actually eaten with its calories, which
//! lands in the day's intake log (`fitness_proto::intake`) — meal prep
//! and calorie tracking from the same place you see the day.

use chrono::NaiveDate;
use dioxus::prelude::*;
use fitness_proto::intake::{IntakeEntry, IntakeLog, IntakeSource};
use task_ui_core::sheet::BottomSheet;
use uuid::Uuid;

/// The slot a block's label names: "Lunch" → "lunch".
#[must_use]
pub fn slot_of(label: &str) -> String {
    label.trim().to_lowercase()
}

/// Whole calories of an entry, when it has them.
fn kcal(e: &IntakeEntry) -> Option<f64> {
    e.nutrition.as_ref().and_then(|n| n.calories)
}

/// A logged line: what was eaten, in this slot, with its calories.
#[must_use]
pub fn eaten_entry(
    name: &str,
    slot: &str,
    calories: Option<f64>,
    recipe: Option<&str>,
) -> IntakeEntry {
    IntakeEntry {
        id: Uuid::new_v4(),
        source: recipe.map_or(IntakeSource::Freeform, |p| IntakeSource::Recipe {
            path: p.to_owned(),
        }),
        name: name.trim().to_owned(),
        qty: 1.0,
        unit: "serving".to_owned(),
        time: Some(chrono::Local::now().time()),
        slot: Some(slot.to_owned()),
        nutrition: calories.map(|c| cookbook_proto::Nutrition {
            calories: Some(c),
            ..cookbook_proto::Nutrition::default()
        }),
        note: None,
    }
}

#[component]
pub fn MealSheet(
    slug: String,
    date: NaiveDate,
    label: String,
    on_close: EventHandler<()>,
) -> Element {
    let slot = slot_of(&label);
    let mut meals = {
        let slug = slug.clone();
        use_resource(move || {
            let slug = slug.clone();
            async move {
                crate::feeds::fetch_meal_plans(&slug)
                    .await
                    .unwrap_or_default()
            }
        })
    };
    let recipes = {
        let slug = slug.clone();
        use_resource(move || {
            let slug = slug.clone();
            async move { crate::feeds::fetch_recipes(&slug).await.unwrap_or_default() }
        })
    };
    let mut intake = {
        let slug = slug.clone();
        use_resource(move || {
            let slug = slug.clone();
            async move {
                crate::feeds::fetch_intake_day(&slug, &date.to_string())
                    .await
                    .ok()
            }
        })
    };
    let mut query = use_signal(String::new);
    let mut ate = use_signal(String::new);
    let mut ate_kcal = use_signal(String::new);
    let mut busy = use_signal(|| None::<String>);

    let planned: Vec<mealplan_proto::Meal> = meals
        .read()
        .as_ref()
        .map(|all| {
            all.iter()
                .filter(|m| m.scheduled_for == date && m.slot.eq_ignore_ascii_case(&slot))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let log: Option<IntakeLog> = intake.read().clone().flatten();
    let eaten: Vec<IntakeEntry> = log
        .as_ref()
        .map(|l| {
            l.entries
                .0
                .iter()
                .filter(|e| {
                    e.slot
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&slot))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let day_kcal: f64 = log
        .as_ref()
        .map(|l| l.entries.0.iter().filter_map(kcal).fold(0.0, |a, k| a + k))
        .unwrap_or(0.0);
    let q = query.read().trim().to_lowercase();
    let matches: Vec<cookbook_proto::Recipe> = recipes
        .read()
        .as_ref()
        .map(|all| {
            all.iter()
                .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q))
                .take(12)
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    // Record something eaten in this slot on the day's log.
    let record = {
        let slug = slug.clone();
        let slot = slot.clone();
        use_callback(move |entry: IntakeEntry| {
            let Some(mut log) = intake.peek().clone().flatten() else {
                return;
            };
            log.entries.0.push(entry);
            let slug = slug.clone();
            busy.set(Some("Saving…".to_owned()));
            let _ = &slot;
            spawn(async move {
                let _ = crate::feeds::save_intake(&slug, log).await;
                busy.set(None);
                intake.restart();
            });
        })
    };
    let plan = {
        let slug = slug.clone();
        let slot = slot.clone();
        use_callback(move |r: cookbook_proto::Recipe| {
            let m = mealplan_proto::Meal {
                path: String::new(),
                id: Uuid::new_v4(),
                name: r.name.clone(),
                scheduled_for: date,
                slot: slot.clone(),
                servings: 1,
                recipe_paths: mealplan_proto::StringList(vec![r.path.clone()]),
                status: mealplan_proto::Status::Planned.as_str().to_owned(),
                pantry_deductions: mealplan_proto::PantryDeductions::default(),
                tags: mealplan_proto::StringList::default(),
                date_created: None,
                date_modified: None,
                details: String::new(),
            };
            let slug = slug.clone();
            spawn(async move {
                let _ = crate::feeds::create_meal(&slug, m).await;
                meals.restart();
            });
        })
    };
    let unplan = {
        let slug = slug.clone();
        use_callback(move |id: Uuid| {
            let slug = slug.clone();
            spawn(async move {
                let _ = crate::feeds::delete_meal(&slug, &id.to_string()).await;
                meals.restart();
            });
        })
    };

    let title = format!("{label} · {}", date.format("%a %-d %b"));
    let section = "px-1 pb-1 pt-4 text-[12px] font-semibold uppercase tracking-[0.06em] text-muted-foreground/80";
    let row = "flex min-h-12 items-center gap-3 rounded-xl px-2";
    let slot_for_log = slot.clone();

    rsx! {
        BottomSheet {
            open: true,
            title,
            on_close: move |()| on_close.call(()),
            div { "data-testid": "meal-sheet", class: "flex flex-col pb-4",
                div { class: "flex items-baseline justify-between px-1",
                    span { class: "text-[13px] text-muted-foreground",
                        if eaten.is_empty() { "Nothing logged for this meal yet" } else { "Logged for this meal" }
                    }
                    span { class: "text-[13px] tabular-nums text-muted-foreground", "Day: {day_kcal:.0} kcal" }
                }
                // ── Eaten ──
                for e in eaten.iter() {
                    div { key: "{e.id}", class: row,
                        span { class: "flex size-6 shrink-0 items-center justify-center rounded-full bg-emerald-500/20 text-emerald-400",
                            architect_ui::lucide_dioxus::Check { size: 14 }
                        }
                        span { class: "min-w-0 flex-1 truncate text-[16px] text-foreground", "{e.name}" }
                        if let Some(k) = kcal(e) {
                            span { class: "shrink-0 text-[14px] tabular-nums text-muted-foreground", "{k:.0} kcal" }
                        }
                    }
                }
                // ── Log what you ate ──
                div { class: "mt-2 flex items-center gap-2",
                    input {
                        r#type: "text",
                        "data-testid": "meal-ate",
                        class: "h-11 min-w-0 flex-1 rounded-xl bg-foreground/[0.06] px-3 text-[16px] text-foreground outline-none placeholder:text-muted-foreground",
                        placeholder: "What did you eat?",
                        value: "{ate}",
                        oninput: move |e| ate.set(e.value()),
                    }
                    input {
                        r#type: "number",
                        inputmode: "numeric",
                        "data-testid": "meal-kcal",
                        class: "h-11 w-24 shrink-0 rounded-xl bg-foreground/[0.06] px-3 text-[16px] tabular-nums text-foreground outline-none placeholder:text-muted-foreground",
                        placeholder: "kcal",
                        value: "{ate_kcal}",
                        oninput: move |e| ate_kcal.set(e.value()),
                    }
                    button {
                        r#type: "button",
                        "data-testid": "meal-log",
                        class: "h-11 shrink-0 rounded-xl bg-primary px-4 text-[15px] font-semibold text-primary-foreground disabled:opacity-40",
                        disabled: ate.read().trim().is_empty() || log.is_none() || busy.read().is_some(),
                        onclick: move |_| {
                            let name = ate.peek().trim().to_owned();
                            let cal = ate_kcal.peek().trim().parse::<f64>().ok();
                            record.call(eaten_entry(&name, &slot_for_log, cal, None));
                            ate.set(String::new());
                            ate_kcal.set(String::new());
                        },
                        "Log"
                    }
                }
                // ── Planned ──
                if !planned.is_empty() {
                    div { class: section, "Planned" }
                    for m in planned.iter().cloned() {
                        {
                            let id = m.id;
                            let recipe = m.recipe_paths.0.first().cloned();
                            let name = m.name.clone();
                            let slot = slot.clone();
                            rsx! {
                                div { key: "{id}", class: row,
                                    span { class: "flex size-6 shrink-0 items-center justify-center text-muted-foreground",
                                        architect_ui::lucide_dioxus::ChefHat { size: 16 }
                                    }
                                    span { class: "min-w-0 flex-1 truncate text-[16px] text-foreground", "{m.name}" }
                                    button {
                                        r#type: "button",
                                        class: "h-9 shrink-0 rounded-full bg-foreground/[0.08] px-3 text-[14px] text-foreground active:bg-foreground/15",
                                        onclick: move |_| record.call(eaten_entry(&name, &slot, None, recipe.as_deref())),
                                        "Ate this"
                                    }
                                    button {
                                        r#type: "button",
                                        class: "flex size-9 shrink-0 items-center justify-center rounded-full text-muted-foreground active:bg-foreground/10",
                                        aria_label: "Remove from the plan",
                                        onclick: move |_| unplan.call(id),
                                        architect_ui::lucide_dioxus::X { size: 16 }
                                    }
                                }
                            }
                        }
                    }
                }
                // ── Plan from the cookbook ──
                div { class: section, "Plan from the cookbook" }
                input {
                    r#type: "search",
                    class: "mb-1 h-11 rounded-xl bg-foreground/[0.06] px-3 text-[16px] text-foreground outline-none placeholder:text-muted-foreground",
                    placeholder: "Search recipes",
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                }
                if matches.is_empty() {
                    p { class: "px-2 py-3 text-[14px] text-muted-foreground",
                        if recipes.read().is_none() { "Loading recipes…" } else { "No recipes match." }
                    }
                }
                for r in matches.into_iter() {
                    {
                        let course = r.course.clone().unwrap_or_default();
                        let pick = r.clone();
                        rsx! {
                            button {
                                key: "{r.path}",
                                r#type: "button",
                                "data-testid": "meal-recipe",
                                class: "{row} w-full text-left active:bg-accent/60",
                                onclick: move |_| plan.call(pick.clone()),
                                span { class: "flex size-6 shrink-0 items-center justify-center text-primary",
                                    architect_ui::lucide_dioxus::Plus { size: 16 }
                                }
                                span { class: "min-w-0 flex-1 truncate text-[16px] text-foreground", "{r.name}" }
                                if !course.is_empty() {
                                    span { class: "shrink-0 text-[13px] text-muted-foreground", "{course}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_names_its_slot() {
        assert_eq!(slot_of("  Lunch "), "lunch");
    }

    #[test]
    fn a_logged_meal_carries_its_slot_and_calories() {
        let e = eaten_entry("Leftover chilli", "dinner", Some(520.0), None);
        assert_eq!(e.slot.as_deref(), Some("dinner"));
        assert_eq!(kcal(&e), Some(520.0));
        assert!(matches!(e.source, IntakeSource::Freeform));
        let r = eaten_entry(
            "Sourdough",
            "lunch",
            None,
            Some("Cookbook/Sourdough Loaf.cook"),
        );
        assert!(matches!(r.source, IntakeSource::Recipe { .. }));
        assert_eq!(kcal(&r), None);
    }
}
