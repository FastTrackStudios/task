//! `/timer/now` — the running timer, and nothing else.
//!
//! Where a swipe up from the capture button lands (and the composer's
//! Start timer): the clock is already running before you know what it
//! is for, so the page asks — "What are you working on?" — and saves
//! the answer to the session as you type it. Pause keeps the work to
//! come back to, Done stops it, and the day's total sits underneath.
//! A page in the shell, not a takeover: the tab bar stays.

use chrono::Utc;
use dioxus::prelude::*;

use crate::chrome::{fmt_hms, owner_id, resolve_org, use_active_timer, use_second_tick};
use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

/// A start request for "a timer, for whatever this turns out to be".
pub(crate) fn bare_start(
    org_id: uuid::Uuid,
    description: String,
) -> timer_proto::StartTimerRequest {
    timer_proto::StartTimerRequest {
        user_id: owner_id(org_id),
        org_id,
        project_id: None,
        project_path: String::new(),
        task_note_path: String::new(),
        description,
    }
}

/// Start a timer now (or keep the running one) and open its page.
/// `description` may be empty — the page asks for it.
pub(crate) fn use_start_timer_now() -> Callback<String> {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let muts = stores::use_timer_mutations();
    let active = use_active_timer();
    let nav = use_navigator();
    use_callback(move |description: String| {
        if let Some((slug, org_id)) = resolve_org(&selection.peek(), &org_list.peek()) {
            let req = bare_start(org_id, description.clone());
            match active.as_ref() {
                // Already running and nothing new to name: just open it.
                Some(_) if description.is_empty() => {}
                Some(a) => muts.switch(slug, a.session.id, req),
                None => muts.start(slug, req),
            }
        }
        nav.push(crate::routes::Route::TimerNowRoute {});
    })
}

#[component]
pub fn TimerFocus() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let muts = stores::use_timer_mutations();
    let sessions = stores::use_session_list();
    let tick = use_signal(|| 0u64);
    use_second_tick(tick);
    let _ = tick();
    let active = use_active_timer();
    let nav = use_navigator();

    // The description field, seeded from the session once it loads and
    // saved back a moment after typing stops.
    let mut draft = use_signal(String::new);
    let mut seeded_for = use_signal(|| None::<uuid::Uuid>);
    if let Some(a) = active.as_ref() {
        if *seeded_for.peek() != Some(a.session.id) {
            seeded_for.set(Some(a.session.id));
            draft.set(a.session.description.clone());
        }
    }
    let save = {
        let active = active.clone();
        move || {
            let Some(a) = active.as_ref() else { return };
            let text = draft.peek().trim().to_owned();
            if text == a.session.description {
                return;
            }
            muts.update(
                a.slug.clone(),
                timer_proto::service::UpdateSessionRequest {
                    id: a.session.id,
                    user_id: None,
                    project_id: None,
                    project_path: None,
                    task_note_path: None,
                    description: Some(text),
                    start_time: None,
                    end_time: None,
                    billable: None,
                    preserve_rate: false,
                },
            );
        }
    };

    // Today's tracked total in this org, the running one included.
    let today = chrono::Local::now().date_naive();
    let slug = resolve_org(&selection.read(), &org_list.read()).map(|(s, _)| s);
    let today_secs: i64 = sessions
        .value()
        .map(|rows| {
            rows.iter()
                .map(|(_, r)| r)
                .filter(|r| Some(&r.slug) == slug.as_ref())
                .filter(|r| {
                    r.session
                        .start_time
                        .with_timezone(&chrono::Local)
                        .date_naive()
                        == today
                })
                .map(|r| {
                    (r.session.end_time.unwrap_or_else(Utc::now) - r.session.start_time)
                        .num_seconds()
                        .max(0)
                })
                .sum()
        })
        .unwrap_or(0);

    let Some(a) = active.clone() else {
        // Nothing running: one button to start, the rest as on Timer.
        return rsx! {
            div { "data-testid": "timer-focus", class: "mx-auto flex min-h-[70vh] max-w-md flex-col items-center justify-center gap-6 px-6 text-center",
                span { class: "text-[15px] text-muted-foreground", "No timer running · {fmt_hms(today_secs)} today" }
                button {
                    r#type: "button",
                    class: "flex size-28 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-[0_12px_40px_-12px_rgb(0_0_0/0.6)] active:scale-95",
                    aria_label: "Start a timer",
                    onclick: move |_| {
                        if let Some((slug, org_id)) = resolve_org(&selection.read(), &org_list.read()) {
                            muts.start(slug, bare_start(org_id, String::new()));
                        }
                    },
                    architect_ui::lucide_dioxus::Play { size: 40 }
                }
                button {
                    r#type: "button",
                    class: "text-[14px] text-muted-foreground underline-offset-4 active:underline",
                    onclick: move |_| { nav.push(crate::routes::Route::TimerRoute {}); },
                    "All time entries"
                }
            }
        };
    };

    let elapsed = (Utc::now() - a.session.start_time).num_seconds().max(0);
    let started = a
        .session
        .start_time
        .with_timezone(&chrono::Local)
        .format("%-H:%M")
        .to_string();
    let save_blur = save.clone();
    let save_key = save.clone();
    let (stop_slug, stop_session) = (a.slug.clone(), a.session.clone());
    let (pause_slug, pause_session) = (a.slug.clone(), a.session.clone());

    rsx! {
        div { "data-testid": "timer-focus", class: "mx-auto flex min-h-[78vh] w-full max-w-md flex-col px-6 pt-6",
            div { class: "flex items-center gap-2 text-[14px] text-muted-foreground",
                span { class: "size-2 animate-pulse rounded-full bg-rose-400" }
                "Running since {started}"
            }
            // What it is for — say it whenever you know.
            input {
                r#type: "text",
                "data-testid": "timer-focus-what",
                class: "mt-4 w-full bg-transparent text-[26px] font-semibold leading-8 tracking-[-0.01em] text-foreground outline-none placeholder:text-muted-foreground/60",
                placeholder: "What are you working on?",
                value: "{draft}",
                oninput: move |e| draft.set(e.value()),
                onblur: move |_| save_blur(),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter {
                        save_key();
                        let _ = dioxus::document::eval("document.activeElement && document.activeElement.blur()");
                    }
                },
            }
            // The clock.
            div { class: "flex flex-1 flex-col items-center justify-center py-10",
                span { class: "text-[72px] font-semibold tabular-nums leading-none tracking-[-0.03em] text-foreground",
                    "{fmt_hms(elapsed)}"
                }
                span { class: "mt-3 text-[14px] tabular-nums text-muted-foreground", "{fmt_hms(today_secs)} today" }
            }
            div { class: "grid grid-cols-2 gap-3 pb-6",
                button {
                    r#type: "button",
                    "data-testid": "timer-focus-pause",
                    class: "flex h-14 items-center justify-center gap-2 rounded-2xl bg-popover text-[17px] font-semibold text-foreground active:bg-accent",
                    onclick: move |_| {
                        let user = pause_session.user_id;
                        muts.stop(pause_slug.clone(), user, pause_session.id);
                    },
                    architect_ui::lucide_dioxus::Pause { size: 20 }
                    "Pause"
                }
                button {
                    r#type: "button",
                    "data-testid": "timer-focus-done",
                    class: "flex h-14 items-center justify-center gap-2 rounded-2xl bg-primary text-[17px] font-semibold text-primary-foreground active:opacity-90",
                    onclick: move |_| {
                        let user = stop_session.user_id;
                        muts.stop(stop_slug.clone(), user, stop_session.id);
                        nav.push(crate::routes::Route::TimerRoute {});
                    },
                    architect_ui::lucide_dioxus::Check { size: 20 }
                    "Done"
                }
            }
        }
    }
}
