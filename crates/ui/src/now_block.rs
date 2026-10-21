//! The hour you're in, and the mode you choose to work it in.
//!
//! A day plan (`/schedule`) says what each part of the day is for; this
//! module reads which steering block is running now
//! ([`scheduling_proto::steering_block_at`]) and what it brings forward
//! ([`scheduling_proto::BlockCategory::surfaces`]).
//!
//! The hour only **suggests**. During a maintenance block the Tasks
//! page's Suggested slot (`crate::suggested`) offers "Start maintenance
//! mode"; nothing changes on the list
//! until the person says yes. Starting the mode opens a timer session
//! named for it ("Maintenance hour"), and *that session is the mode*:
//! while it runs, the board's relevance context gets the mode's `focus`
//! (so `@errands` tasks are shown at all and rank first) and the
//! [`ModeBand`] lists the mode's work with its running clock. Ending
//! the mode stops the session — so the timer log (and `task plan
//! actual`) records when maintenance actually started and how long it
//! took, not when the plan said it would. A mode can also be started
//! from the timer, by naming the session for it ("Maintenance hour").
//!
//! Read-only on the plan: nothing here writes it. A day with no saved
//! plan falls back to its weekday / weekend template, the same merge the
//! calendar draws.

use chrono::{Local, Timelike, Utc};
use dioxus::prelude::*;
use scheduling_proto::BlockCategory;
use task_proto::TaskInfo as DbTask;
use uuid::Uuid;

use crate::routes::Route;
use crate::stores;

/// A mode the person started: the open timer session named for a
/// steering kind (`BlockCategory::focus_label`).
#[derive(Clone, Debug, PartialEq)]
pub struct FocusMode {
    pub category: BlockCategory,
    /// "Maintenance mode".
    pub name: &'static str,
    /// Context / tag names it brings forward.
    pub focus: Vec<String>,
    /// When the person started it — the session's start.
    pub started: chrono::DateTime<Utc>,
    pub slug: String,
    pub user_id: Uuid,
    pub session_id: Uuid,
}

fn focus_of(category: BlockCategory) -> Vec<String> {
    category
        .surfaces()
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// The mode a running session is, if it is one: its description names
/// a steering kind ("Maintenance hour").
#[must_use]
// t[impl process.focus.mode]
pub fn mode_of(slug: &str, s: &timer_proto::WorkSession) -> Option<FocusMode> {
    if s.end_time.is_some() {
        return None;
    }
    let category = BlockCategory::from_focus_label(&s.description)?;
    Some(FocusMode {
        category,
        name: task_proto::suggest::mode_name(category),
        focus: focus_of(category),
        started: s.start_time,
        slug: slug.to_owned(),
        user_id: s.user_id,
        session_id: s.id,
    })
}

/// The mode running now for the selected org, if one is — reactive on
/// the shared session store, so the board's memos follow Start and End.
pub fn use_focus_mode() -> Memo<Option<FocusMode>> {
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let orgs = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let _sessions = stores::use_session_list();
    let session_store = stores::use_session_store();
    use_memo(move || {
        let (slug, org_id) = crate::chrome::resolve_org(&selection.read(), &orgs.read())?;
        let owner = crate::chrome::owner_id(org_id);
        session_store
            .list()
            .iter()
            .filter(|r| r.slug == slug && r.session.user_id == owner)
            .find_map(|r| mode_of(&slug, &r.session))
    })
}

/// "Now" for every board surface — Home, the Tasks page and the Work
/// sidebar read this one memo, so they pass the same context to the
/// same functions (`process.focus.everywhere`): the clock, where the
/// person says they are, this device (`process.context.device`, via
/// [`crate::task_board::this_device`]), and the person's open session —
/// the timer's project, and the mode's focus when the session is one.
/// The hour alone never changes the list; only a started mode does.
/// Re-read every minute so the clock-gated contexts follow the time.
pub fn use_now_ctx() -> Memo<task_proto::RelevanceContext> {
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let orgs = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let prefs = use_context::<crate::prefs::PrefsCtx>().prefs;
    let phone = try_use_context::<crate::shell::phone_gestures::PhonePanels>().map(|p| p.phone);
    let _sessions = stores::use_session_list();
    let session_store = stores::use_session_store();
    let mut tick = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            architect::sleep(architect::Duration::from_secs(60)).await;
            tick += 1;
        }
    });
    use_memo(move || {
        tick();
        let location = prefs.read().location.clone();
        let device = crate::task_board::this_device(phone.is_some_and(|p| p()));
        let open = crate::chrome::resolve_org(&selection.read(), &orgs.read()).and_then(
            |(slug, org_id)| {
                let owner = crate::chrome::owner_id(org_id);
                session_store
                    .list()
                    .iter()
                    .find(|r| {
                        r.slug == slug && r.session.user_id == owner && r.session.end_time.is_none()
                    })
                    .map(|r| (r.session.description.clone(), r.session.project_id))
            },
        );
        crate::task_board::relevance_ctx(
            &location,
            device,
            open.as_ref().map(|(d, p)| (d.as_str(), *p)),
        )
    })
}

/// What today's plan says about now for the selected org — the task it
/// planned and the steering block's mode (`task_proto::suggest::now_hint`)
/// — re-read every minute. `None` when nothing is planned now (or plans
/// haven't loaded).
pub fn use_now_hint() -> Memo<Option<task_proto::suggest::NowHint>> {
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
        task_proto::suggest::now_hint(&row.plan.blocks.0, minute)
    })
}

/// Open tasks a focus brings forward, in-progress first, then soonest due.
#[must_use]
pub fn tasks_for(focus: &[String], tasks: &[&DbTask]) -> Vec<DbTask> {
    if focus.is_empty() {
        return Vec::new();
    }
    let ctx = task_proto::RelevanceContext {
        focus: focus.to_vec(),
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

/// How many of the mode's tasks the band lists before "N more".
const BAND_ROWS: usize = 4;

/// The running mode's band: its name, running clock, End, and the tasks
/// it is for. Renders nothing without a mode — the suggestion to start
/// one is the Tasks page's Suggested slot (`crate::suggested`).
///
/// `tasks` is every task the band may draw from; it filters to the
/// mode's focus itself.
#[component]
pub fn ModeBand(
    mode: Option<FocusMode>,
    tasks: Vec<DbTask>,
    on_status: EventHandler<(uuid::Uuid, String)>,
) -> Element {
    let timers = stores::use_timer_mutations();
    let tick = use_signal(|| 0u64);
    crate::chrome::use_second_tick(tick);

    if let Some(m) = mode {
        let _ = tick();
        let all: Vec<&DbTask> = tasks.iter().collect();
        let list = tasks_for(&m.focus, &all);
        let more = list.len().saturating_sub(BAND_ROWS);
        let names = describe(&m.focus);
        let elapsed = crate::chrome::fmt_hms((Utc::now() - m.started).num_seconds().max(0));
        let (slug, user, session) = (m.slug.clone(), m.user_id, m.session_id);
        return rsx! {
            // The Suggested slot's language (`crate::suggested`), live: a
            // header line with the pulse, name and clock, then the same
            // lifted surface holding the mode's work.
            section { class: "flex flex-col gap-2", aria_label: "{m.name}",
                div { class: "flex items-center gap-2 px-1 text-[13px] leading-5",
                    span { class: "relative flex size-2 shrink-0",
                        span { class: "absolute inline-flex size-full animate-ping rounded-full bg-primary opacity-60" }
                        span { class: "relative inline-flex size-2 rounded-full bg-primary" }
                    }
                    span { class: "font-medium text-foreground/90", "{m.name}" }
                    span { class: "text-muted-foreground/60", "·" }
                    span { class: "min-w-0 flex-1 font-mono text-[12px] tabular-nums text-muted-foreground", "{elapsed}" }
                    button {
                        class: "flex h-7 shrink-0 items-center rounded-full bg-foreground/[0.07] px-3 text-[12px] font-semibold text-foreground shadow-[inset_0_0_0_1px_rgb(255_255_255/0.08)] transition-transform duration-150 active:scale-95",
                        onclick: move |_| timers.stop(slug.clone(), user, session),
                        "End"
                    }
                }
                div { class: "relative isolate flex flex-col overflow-hidden rounded-2xl bg-gradient-to-br from-primary/[0.14] via-primary/[0.04] to-foreground/[0.02] px-3 py-1.5 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.06),0_10px_30px_-18px_rgb(0_0_0/0.8)]",
                    span { class: "pointer-events-none absolute -left-10 -top-12 -z-10 size-36 rounded-full bg-primary/20 blur-3xl" }
                    if list.is_empty() {
                        p { class: "py-2.5 text-[13px] leading-5 text-muted-foreground",
                            "Nothing tagged {names} is waiting. Tag a task with one of those and it shows up here."
                        }
                    } else {
                        div { class: "flex flex-col divide-y divide-foreground/[0.06]",
                            for t in list.into_iter().take(BAND_ROWS) {
                                BandRow { key: "{t.id}", task: t, on_status }
                            }
                        }
                        if more > 0 {
                            Link {
                                to: Route::TasksRoute {},
                                class: "self-start py-2 text-[12px] text-muted-foreground hover:text-foreground",
                                "{more} more for this mode →"
                            }
                        }
                    }
                }
            }
        };
    }

    rsx! {}
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
                class: "min-w-0 flex-1 truncate text-[15px] leading-[22px] tracking-[-0.006em] text-foreground",
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

    fn focus() -> Vec<String> {
        focus_of(BlockCategory::Maintenance)
    }

    #[test]
    fn the_band_lists_what_the_mode_is_for_working_first() {
        let mut errand = task_proto::capture("Post the parcel @errands");
        errand.title = "Post the parcel".into();
        let mut chore = task_proto::capture("Mop #chore");
        chore.status = "in-progress".into();
        let mix = task_proto::capture("Mix track two");
        let mut done = task_proto::capture("Old errand @errands");
        done.status = "done".into();
        let all = vec![&errand, &chore, &mix, &done];
        let got: Vec<String> = tasks_for(&focus(), &all)
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(got, vec!["Mop".to_owned(), "Post the parcel".to_owned()]);
        assert!(tasks_for(&[], &all).is_empty(), "no mode, no band");
    }

    #[test]
    // t[verify process.focus.mode]
    fn only_an_open_session_named_for_a_kind_is_a_mode() {
        let mut s = timer_proto::WorkSession {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            project_id: None,
            project_path: String::new(),
            description: "Maintenance hour".into(),
            start_time: Utc::now(),
            end_time: None,
            billable: false,
            rate_cents: 0,
            currency: String::new(),
            task_note_path: String::new(),
            invoice_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let m = mode_of("acme", &s).expect("a running maintenance session is the mode");
        assert_eq!(m.name, "Maintenance mode");
        assert!(m.focus.iter().any(|f| f == "errands"));
        s.end_time = Some(Utc::now());
        assert!(mode_of("acme", &s).is_none(), "ended, the mode is over");
        s.end_time = None;
        s.description = "Mix track two".into();
        assert!(mode_of("acme", &s).is_none(), "ordinary work is not a mode");
    }

    #[test]
    fn the_band_names_the_contexts_once_each() {
        assert_eq!(describe(&focus()), "@errands, @chores or @home");
    }
}
