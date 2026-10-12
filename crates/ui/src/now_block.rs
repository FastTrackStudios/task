//! The hour you're in, and the tasks it is for.
//!
//! A day plan (`/schedule`) says what each part of the day is for; this
//! module reads which steering block is running now
//! ([`scheduling_proto::steering_block_at`]) and what it brings forward
//! ([`scheduling_proto::BlockCategory::surfaces`]). During a maintenance
//! hour that is errands and chores: the Tasks page and Home lead with
//! them in a [`NowBlockBand`], and the board's relevance context gets
//! the block's `focus` so `@errands` tasks are shown at all.
//!
//! Read-only: nothing here writes the plan. A day with no saved plan
//! falls back to its weekday / weekend template, the same merge the
//! calendar draws.

use architect_ui::lucide_dioxus::Clock;
use chrono::{Local, Timelike};
use dioxus::prelude::*;
use task_proto::TaskInfo as DbTask;

use crate::routes::Route;
use crate::stores;

/// The steering block running now.
#[derive(Clone, Debug, PartialEq)]
pub struct NowBlock {
    /// The block's own label ("Errands & chores").
    pub label: String,
    /// What the hour is called on the task list ("Maintenance hour").
    pub title: &'static str,
    /// When it ends, as the clock says it ("5:00 PM").
    pub until: String,
    /// Context / tag names it brings forward (no sigil, lowercase).
    pub focus: Vec<String>,
}

/// The current steering block for the selected org, re-read every
/// minute. `None` when no block steers the hour (or plans haven't
/// loaded).
pub fn use_now_block() -> Memo<Option<NowBlock>> {
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let orgs = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &orgs.read())
            .into_iter()
            .next()
    });
    let templates = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_day_templates(&s).await.ok(),
            None => None,
        }
    });
    let tpl_list = use_memo(move || templates.read().clone().flatten());
    let today = Local::now().date_naive();
    // Drives the fetch into the shared day-plan store; the memo below
    // reads the store, so a plan edited on /schedule shows here too.
    let _plans = stores::use_dayplan_list(Some((today, today)), tpl_list());
    let plan_store = stores::use_dayplan_store();

    let mut tick = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            architect::sleep(architect::Duration::from_secs(60)).await;
            tick += 1;
        }
    });

    use_memo(move || {
        tick();
        let now = Local::now();
        let today = now.date_naive();
        let minute = u16::try_from(now.hour() * 60 + now.minute()).ok()?;
        let rows = plan_store.list();
        let row = rows.iter().find(|r| r.date == today)?;
        let block = scheduling_proto::steering_block_at(&row.plan.blocks.0, minute)?;
        let end = block.end;
        let until =
            chrono::NaiveTime::from_hms_opt(u32::from(end.hours()), u32::from(end.minutes()), 0)
                .map(|t| t.format("%-I:%M %p").to_string())
                .unwrap_or_default();
        Some(NowBlock {
            label: block.label.clone(),
            title: block.category.focus_label().unwrap_or("This hour"),
            until,
            focus: block
                .category
                .surfaces()
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        })
    })
}

/// Open tasks the block is for, in-progress first, then soonest due.
#[must_use]
pub fn tasks_for(block: &NowBlock, tasks: &[&DbTask]) -> Vec<DbTask> {
    let ctx = task_proto::RelevanceContext {
        focus: block.focus.clone(),
        ..Default::default()
    };
    let mut out: Vec<DbTask> = tasks
        .iter()
        .filter(|t| task_proto::status_is_open(&t.status) && task_proto::matches_focus(t, &ctx))
        .map(|t| (*t).clone())
        .collect();
    out.sort_by(|a, b| {
        let working = |t: &DbTask| t.status != "in-progress";
        working(a)
            .cmp(&working(b))
            .then_with(|| a.due.is_none().cmp(&b.due.is_none()))
            .then_with(|| a.due.cmp(&b.due))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    out
}

/// How many of the block's tasks the band lists before "N more".
const BAND_ROWS: usize = 4;

/// "Maintenance hour · until 5:00 PM", and the tasks it is for — a
/// thumb's reach from done. Renders nothing when no block steers.
#[component]
pub fn NowBlockBand(
    block: Option<NowBlock>,
    tasks: Vec<DbTask>,
    on_status: EventHandler<(uuid::Uuid, String)>,
) -> Element {
    let Some(block) = block else {
        return rsx! {};
    };
    let more = tasks.len().saturating_sub(BAND_ROWS);
    let names = describe(&block.focus);
    rsx! {
        section {
            class: "flex flex-col gap-2 rounded-xl border border-primary/30 bg-primary/[0.07] p-3.5",
            aria_label: "{block.title}",
            div { class: "flex items-center gap-2",
                span { class: "text-primary", Clock { size: 16 } }
                span { class: "text-sm font-semibold text-foreground", "{block.title}" }
                span { class: "text-xs text-muted-foreground", "until {block.until}" }
            }
            if tasks.is_empty() {
                p { class: "text-sm text-muted-foreground",
                    "Nothing tagged {names} is waiting. Tag a task with one of those and it shows up here in this hour."
                }
            } else {
                div { class: "flex flex-col",
                    for t in tasks.into_iter().take(BAND_ROWS) {
                        BandRow { key: "{t.id}", task: t, on_status }
                    }
                }
                if more > 0 {
                    Link {
                        to: Route::TasksRoute {},
                        class: "self-start text-xs text-muted-foreground hover:text-foreground",
                        "{more} more for this hour →"
                    }
                }
            }
        }
    }
}

/// "@errands, @chores or @home" — the band's way of saying what counts.
fn describe(focus: &[String]) -> String {
    // The plural/singular pairs read once; tags are spelled as contexts.
    let mut names: Vec<String> = Vec::new();
    for f in focus {
        let base = f.trim_end_matches('s');
        if !names
            .iter()
            .any(|n| n.trim_start_matches('@').trim_end_matches('s') == base)
        {
            names.push(format!("@{f}"));
        }
    }
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        _ => {
            let shown: Vec<String> = names.iter().take(3).cloned().collect();
            let (last, rest) = shown.split_last().expect("non-empty");
            format!("{} or {last}", rest.join(", "))
        }
    }
}

/// One task in the band: live checkbox, title, due.
#[component]
fn BandRow(task: DbTask, on_status: EventHandler<(uuid::Uuid, String)>) -> Element {
    let id = task.id;
    let status = task.status.clone();
    let ui_status = task_ui::Status::from_str(&task.status).unwrap_or(task_ui::Status::Open);
    let ui_priority =
        task_ui::Priority::from_str(&task.priority).unwrap_or(task_ui::Priority::Normal);
    let contexts: Vec<String> = task.contexts.iter().cloned().collect();
    rsx! {
        div { class: "flex min-h-11 items-center gap-2.5 py-1",
            task_ui::CheckboxButton {
                status: ui_status,
                priority: ui_priority,
                title: task.title.clone(),
                on_click: move |()| {
                    on_status.call((id, task_proto::click_transition(&status, None).to_string()));
                },
            }
            Link {
                to: Route::TaskDetailRoute { id },
                class: "min-w-0 flex-1 truncate text-sm text-foreground hover:underline",
                "{task.title}"
            }
            for c in contexts.into_iter().take(1) {
                span { class: "shrink-0 rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground", "{c}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> NowBlock {
        NowBlock {
            label: "Errands".into(),
            title: "Maintenance hour",
            until: "5:00 PM".into(),
            focus: scheduling_proto::BlockCategory::Maintenance
                .surfaces()
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        }
    }

    #[test]
    fn the_band_lists_what_the_hour_is_for_working_first() {
        let mut errand = task_proto::capture("Post the parcel @errands");
        errand.title = "Post the parcel".into();
        let mut chore = task_proto::capture("Mop #chore");
        chore.status = "in-progress".into();
        let mix = task_proto::capture("Mix track two");
        let mut done = task_proto::capture("Old errand @errands");
        done.status = "done".into();
        let all = vec![&errand, &chore, &mix, &done];
        let got: Vec<String> = tasks_for(&block(), &all)
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(got, vec!["Mop".to_owned(), "Post the parcel".to_owned()]);
    }

    #[test]
    fn the_band_names_the_contexts_once_each() {
        assert_eq!(describe(&block().focus), "@errands, @chores or @home");
    }
}
