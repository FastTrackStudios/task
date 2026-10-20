//! The Tasks page's **Suggested** slot: the one thing most worth doing
//! now. Which thing, and the words for it, are `task_proto::suggest` —
//! shared with the watch, so the wrist and the phone agree — read from
//! today's plan (`crate::now_block::use_now_hint`), the person's own
//! tasks, the running timer and the inbox.
//!
//! The slot only acts on a tap: Start opens a timer session (on the
//! planned task, or named for the block's mode), Done completes, Process
//! opens the inbox. "Not now" sets the suggestion aside for the day on
//! this device and the next one shows. While a mode runs the slot is the
//! mode's own band instead (`crate::now_block::ModeBand`).

use architect_ui::lucide_dioxus::{
    AlarmClock, ArrowRight, CalendarCheck, Check, Clock, Inbox, Play, Sparkles, X,
};
use chrono::Local;
use dioxus::prelude::*;
use task_proto::TaskInfo;
use task_proto::suggest::{NowHint, SuggestInput, Suggestion, suggest};

use crate::routes::Route;
use crate::stores;

/// Where "Not now" lives: `<date>|key|key…`, reset each day.
const DECLINED_KEY: &str = "task.suggested.declined";

fn load_declined(today: &str) -> Vec<String> {
    let raw = crate::local_store::get(DECLINED_KEY).unwrap_or_default();
    let mut parts = raw.split('|');
    if parts.next() != Some(today) {
        return Vec::new();
    }
    parts.map(str::to_owned).collect()
}

fn save_declined(today: &str, keys: &[String]) {
    let mut raw = today.to_owned();
    for k in keys {
        raw.push('|');
        raw.push_str(k);
    }
    crate::local_store::set(DECLINED_KEY, &raw);
}

/// The slot. Renders nothing when there is nothing worth suggesting.
#[component]
pub fn Suggested(
    hint: Option<NowHint>,
    mine: Vec<TaskInfo>,
    on_status: EventHandler<(uuid::Uuid, String)>,
) -> Element {
    let today = Local::now().date_naive().to_string();
    let to_process = crate::process::use_to_process_count();
    let mut declined = use_signal({
        let today = today.clone();
        move || load_declined(&today)
    });
    let timers = stores::use_timer_mutations();
    let active = crate::chrome::use_active_timer();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let orgs = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let nav = use_navigator();

    // The task a running timer is on, by its note path.
    let timing = active.as_ref().and_then(|at| {
        let path = &at.session.task_note_path;
        mine.iter()
            .find(|t| !path.is_empty() && t.path == *path)
            .map(|t| t.id)
    });
    let refs: Vec<&TaskInfo> = mine.iter().collect();
    let Some(s) = suggest(&SuggestInput {
        now: hint.as_ref(),
        mine: &refs,
        timing,
        to_process,
        today: &today,
        declined: &declined.read(),
    }) else {
        return rsx! {};
    };
    let card = s.card();
    let key = s.key();
    let not_now = move |_| {
        let mut keys = declined.peek().clone();
        keys.push(key.clone());
        save_declined(&today, &keys);
        declined.set(keys);
    };
    let icon = match &s {
        Suggestion::StartPlanned { .. } => rsx! { Play { size: 20 } },
        Suggestion::StartMode { .. } => rsx! { Clock { size: 20 } },
        Suggestion::Continue(_) => rsx! { Play { size: 20 } },
        Suggestion::Overdue(_) => rsx! { AlarmClock { size: 20 } },
        Suggestion::Process(_) => rsx! { Inbox { size: 20 } },
        Suggestion::DueToday(_) => rsx! { CalendarCheck { size: 20 } },
    };
    let task = s.task().cloned();

    let go = {
        let s = s.clone();
        move |_| {
            let start = |description: String, task: Option<&TaskInfo>| {
                let Some((slug, org_id)) =
                    crate::chrome::resolve_org(&selection.read(), &orgs.read())
                else {
                    return;
                };
                let req = timer_proto::StartTimerRequest {
                    user_id: crate::chrome::owner_id(org_id),
                    org_id,
                    project_id: task.and_then(|t| t.project_id),
                    project_path: String::new(),
                    task_note_path: task.map(|t| t.path.clone()).unwrap_or_default(),
                    description,
                };
                // Whatever was running stops where this starts.
                match active.as_ref() {
                    Some(at) => timers.switch(slug, at.session.id, req),
                    None => timers.start(slug, req),
                }
            };
            match &s {
                Suggestion::StartPlanned { task, .. } => {
                    start(task.title.clone(), Some(task));
                    on_status.call((task.id, "in-progress".to_owned()));
                }
                Suggestion::StartMode { mode, .. } => start(mode.title.clone(), None),
                Suggestion::Process(_) => {
                    nav.push(Route::InboxRoute {});
                }
                Suggestion::Continue(t) | Suggestion::Overdue(t) | Suggestion::DueToday(t) => {
                    on_status.call((t.id, "done".to_owned()));
                }
            }
        }
    };

    let action_icon = match card.action {
        "Start" => rsx! { Play { size: 14 } },
        "Process" => rsx! { ArrowRight { size: 14 } },
        _ => rsx! { Check { size: 14 } },
    };
    let text = rsx! {
        span { class: "line-clamp-2 text-[16px] font-semibold leading-[22px] tracking-[-0.011em] text-foreground",
            "{card.title}"
        }
        if !card.detail.is_empty() {
            span { class: "mt-0.5 truncate text-[13px] leading-[18px] text-muted-foreground", "{card.detail}" }
        }
    };

    // Linear-style: a quiet header line carrying why-now, then one
    // lifted surface — soft primary wash, inset hairline, an icon tile,
    // the title at reading size and a single pill action. Not now is
    // the × in the header, out of the thumb's main path.
    rsx! {
        section { class: "flex flex-col gap-2", aria_label: "Suggested",
            div { class: "flex items-center gap-1.5 px-1 text-[13px] leading-5",
                span { class: "text-primary", Sparkles { size: 14 } }
                span { class: "font-medium text-foreground/90", "Suggested" }
                span { class: "text-muted-foreground/60", "·" }
                span { class: "min-w-0 flex-1 truncate text-muted-foreground", "{card.eyebrow}" }
                button {
                    class: "-mr-1 flex size-7 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/[0.06] hover:text-foreground active:bg-foreground/10",
                    aria_label: "Not now",
                    title: "Not now",
                    onclick: not_now,
                    X { size: 15 }
                }
            }
            div { class: "relative isolate flex items-center gap-3.5 overflow-hidden rounded-2xl bg-gradient-to-br from-primary/[0.16] via-primary/[0.05] to-foreground/[0.02] p-4 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.06),0_10px_30px_-18px_rgb(0_0_0/0.8)]",
                // A soft glow behind the icon — depth without a border.
                span { class: "pointer-events-none absolute -left-10 -top-12 -z-10 size-36 rounded-full bg-primary/25 blur-3xl" }
                span { class: "flex size-10 shrink-0 items-center justify-center rounded-xl bg-primary/15 text-primary shadow-[inset_0_0_0_1px_rgb(255_255_255/0.08)]",
                    {icon}
                }
                if let Some(t) = task {
                    Link {
                        to: Route::TaskDetailRoute { id: t.id },
                        class: "flex min-w-0 flex-1 flex-col",
                        {text}
                    }
                } else {
                    div { class: "flex min-w-0 flex-1 flex-col", {text} }
                }
                button {
                    class: "flex h-9 shrink-0 items-center gap-1.5 rounded-full bg-primary pl-3 pr-3.5 text-[13px] font-semibold text-primary-foreground shadow-[inset_0_1px_0_rgb(255_255_255/0.2),0_6px_16px_-6px_var(--primary)] transition-transform duration-150 active:scale-95",
                    onclick: go,
                    {action_icon}
                    "{card.action}"
                }
            }
        }
    }
}
