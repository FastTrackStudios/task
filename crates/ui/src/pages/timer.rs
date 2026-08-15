//! `/timer` — billable time tracking over the org's `TimerService`.
//!
//! Shows the running session with a live elapsed clock + a Stop
//! button, a Start form when nothing is running, and a list of recent
//! sessions with a today total. Org-scoped: it reads/writes the
//! selected org's timer service.
//!
//! The Start form's input doubles as a fuzzy task picker: typing
//! filters the shared task store's open tasks ([`crate::fuzzy`]), and
//! picking one links the session to the task's note (`task_note_path`)
//! and project (`project_id` / `project_path`). Plain text still
//! starts an unlinked session.
//!
//! State is the shared optimistic session store ([`crate::stores`]):
//! one slug-tagged list across the selected orgs, with the running
//! session *derived* from it (the open row for the active org +
//! owner) — no separate `active_timer` round-trip and no
//! refresh-counter refetch after start/stop/edit/delete. Mutations
//! patch the store instantly and reconcile against the server
//! (rollback + tray notification on failure).
//!
//! Identity: `org_id` comes from the org's manifest (surfaced via the
//! well-known endpoint). `user_id` is a stable per-org "local owner"
//! id derived from `org_id` — a single-user stand-in until auth wires
//! a real signed-in user through to the page.

use architect::Id;
use chrono::{TimeZone, Utc};
use dioxus::prelude::*;
use architect_ui::prelude::*;
use timer_proto::{LogSessionRequest, StartTimerRequest, WorkSession};
use uuid::Uuid;

use crate::chrome::{fmt_hms, owner_id, resolve_org, use_second_tick};
use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

/// Shared input styling for the manual-log form.
const FIELD: &str = "rounded-md border border-border bg-background px-2.5 py-1.5 text-sm outline-none focus:ring-2 focus:ring-primary/40";

#[component]
pub fn TimerView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));

    // The shared session store: every selected org, newest first.
    let result = stores::use_session_list();
    let muts = stores::use_timer_mutations();

    // Live elapsed clock — re-render once a second while mounted.
    let tick = use_signal(|| 0u64);
    use_second_tick(tick);
    let _ = tick(); // subscribe so the running card's clock ticks.

    let mut description = use_signal(String::new);
    // The task the next session is tracked against, if one was picked
    // from the dropdown. Plain-text starts leave this `None`.
    let mut picked = use_signal(|| None::<PickedTask>);

    // Shared org-wide stores backing the task picker: candidates come
    // from the task list, the `#project` chip from the project list.
    let tasks = stores::use_task_list();
    let projects = stores::use_project_list();

    let store = stores::use_session_store();
    let rows: Vec<(Id<Uuid>, stores::OrgSession)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    // The running session, derived from the one list: the open row for
    // the active org + owner (the backend enforces at most one).
    let active: Option<stores::OrgSession> = target().and_then(|(slug, org_id)| {
        let owner = owner_id(org_id);
        rows.iter()
            .map(|(_, r)| r)
            .find(|r| r.slug == slug && r.session.user_id == owner && r.session.end_time.is_none())
            .cloned()
    });

    // Fuzzy-matched dropdown candidates: open tasks in the active org
    // whose title matches the typed text, best score first, top 8.
    // Hidden once a task is picked (typing then only edits the
    // description) and while the input is empty.
    let candidates: Vec<TaskCandidate> = {
        let query = description();
        let query = query.trim();
        if query.is_empty() || picked.read().is_some() {
            Vec::new()
        } else {
            let target_slug = target().map(|(slug, _)| slug);
            let project_rows = projects.value().cloned().unwrap_or_default();
            let mut scored: Vec<(i64, stores::OrgTask)> = tasks
                .value()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(_, row)| row)
                .filter(|row| is_open_status(&row.task.status))
                .filter(|row| target_slug.as_deref().is_none_or(|s| row.slug == s))
                .filter_map(|row| {
                    crate::fuzzy::fuzzy_match(query, &row.task.title).map(|score| (score, row))
                })
                .collect();
            scored.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then_with(|| a.1.task.title.cmp(&b.1.task.title))
            });
            scored.truncate(8);
            scored
                .into_iter()
                .map(|(_, row)| {
                    let project = row.task.project_id.and_then(|pid| {
                        project_rows
                            .iter()
                            .map(|(_, p)| p)
                            .find(|p| p.project.id == pid)
                    });
                    TaskCandidate {
                        id: row.task.id,
                        title: row.task.title.clone(),
                        path: row.task.path.clone(),
                        project_id: row.task.project_id,
                        project_path: project.map(|p| p.project.path.clone()).unwrap_or_default(),
                        project_title: project.map(|p| p.project.title.clone()),
                    }
                })
                .collect()
        }
    };

    let start = move |_| {
        let Some((slug, org_id)) = target() else {
            return;
        };
        let desc = description.peek().trim().to_string();
        let pick = picked.peek().clone();
        description.set(String::new());
        picked.set(None);
        // A picked task links the session to its note *and* carries the
        // task's project, so project reports pick the session up even
        // when the task belongs to a different project than the last
        // manual start.
        let (project_id, project_path, task_note_path) = pick.map_or_else(
            || (None, String::new(), String::new()),
            |p| (p.project_id, p.project_path, p.path),
        );
        muts.start(
            slug,
            StartTimerRequest {
                user_id: owner_id(org_id),
                org_id,
                project_id,
                project_path,
                task_note_path,
                description: desc,
            },
        );
    };

    // Shared "paused timer" hint (also drives the top-bar widget) — so
    // pausing here shows as paused everywhere, and resuming re-opens the
    // same work.
    let mut hint = crate::chrome::use_resume_hint();

    let active_for_stop = active.clone();
    let stop = move |_| {
        let Some((slug, org_id)) = target() else {
            return;
        };
        let Some(open) = active_for_stop.as_ref() else {
            return;
        };
        muts.stop(slug, owner_id(org_id), open.session.id);
        hint.set(None);
    };

    // Pause: log the current segment and stash its context for resume.
    let active_for_pause = active.clone();
    let pause = move |_| {
        let Some((slug, org_id)) = target() else {
            return;
        };
        let Some(open) = active_for_pause.as_ref() else {
            return;
        };
        let s = &open.session;
        let banked = (Utc::now() - s.start_time).num_seconds().max(0);
        muts.stop(slug.clone(), owner_id(org_id), s.id);
        hint.set(Some(crate::chrome::PausedTimer {
            slug,
            org_id,
            req: crate::chrome::req_from_session(org_id, s),
            title: crate::chrome::session_title(s),
            banked,
        }));
    };

    let resume = move |_| {
        let Some(p) = hint.peek().clone() else {
            return;
        };
        muts.start(p.slug.clone(), p.req.clone());
        hint.set(None);
    };

    let body = if target().is_none() {
        rsx! {
            Text { variant: TextVariant::Muted, "Select an org to track time." }
        }
    } else {
        rsx! {
            // Running session, the paused card, or the start form.
            if let Some(open) = active.as_ref() {
                {running_card(&open.session, stop, pause)}
            } else if let Some(p) = hint.read().clone() {
                {paused_card(p, resume, move |_| hint.set(None))}
            } else if first_load {
                crate::states::LoadingState {}
            } else if let Some(err) = load_err.clone() {
                crate::states::ErrorState {
                    title: "Couldn't reach the timer service",
                    message: err,
                    on_retry: move |()| store.reload(),
                }
            } else {
                {start_form(description, picked, candidates, start)}
            }
            // Manual entry: log time you forgot to track, or work with no
            // specific clock time (just a date + duration).
            if let Some((slug, org_id)) = target() {
                LogPastForm { slug, org_id }
            }
            // Recent sessions (all selected orgs). Loading + service-error
            // are owned by the running-card/start-form block above; here we
            // only distinguish the empty list from a populated one.
            if !first_load && load_err.is_none() {
                if rows.is_empty() {
                    crate::states::EmptyState {
                        title: "No sessions yet",
                        hint: "Start the timer above to log your first session.",
                    }
                } else {
                    {session_list(&rows)}
                }
            }
        }
    };

    rsx! {
        div { class: "mx-auto flex w-full max-w-2xl flex-col gap-5 p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Time tracking"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Timer" }
            }
            {body}
        }
    }
}

/// The running-session card: description, live elapsed clock, Pause +
/// Stop. Sky styling matches the top-bar widget and the task-list Now
/// bar — one visual language for "tracking".
fn running_card(
    s: &WorkSession,
    on_stop: impl FnMut(MouseEvent) + 'static,
    on_pause: impl FnMut(MouseEvent) + 'static,
) -> Element {
    let elapsed = (Utc::now() - s.start_time).num_seconds();
    let title = if s.description.trim().is_empty() {
        "(no description)".to_string()
    } else {
        s.description.clone()
    };
    rsx! {
        div { class: "flex flex-col gap-3 rounded-2xl border border-sky-500/40 bg-sky-500/10 p-5",
            div { class: "flex items-center gap-2",
                span { class: "relative flex size-2.5",
                    span { class: "absolute inline-flex size-full animate-ping rounded-full bg-sky-400/70" }
                    span { class: "relative inline-flex size-2.5 rounded-full bg-sky-400" }
                }
                Text { variant: TextVariant::Muted, "Tracking" }
            }
            div { class: "font-mono text-4xl font-semibold tabular-nums tracking-tight text-sky-100", "{fmt_hms(elapsed)}" }
            div { class: "text-sm text-foreground", "{title}" }
            div { class: "flex gap-2",
                Button {
                    variant: ButtonVariant::Secondary,
                    on_click: on_pause,
                    "Pause"
                }
                Button {
                    variant: ButtonVariant::Destructive,
                    on_click: on_stop,
                    "Stop"
                }
            }
        }
    }
}

/// The paused card: banked time frozen, a Resume that re-opens the same
/// work, and a Discard that drops the pause. Amber to read as "held".
fn paused_card(
    p: crate::chrome::PausedTimer,
    on_resume: impl FnMut(MouseEvent) + 'static,
    on_discard: impl FnMut(MouseEvent) + 'static,
) -> Element {
    rsx! {
        div { class: "flex flex-col gap-3 rounded-2xl border border-amber-500/40 bg-amber-500/10 p-5",
            div { class: "flex items-center gap-2",
                span { class: "size-2.5 rounded-full bg-amber-400" }
                Text { variant: TextVariant::Muted, "Paused" }
            }
            div { class: "font-mono text-4xl font-semibold tabular-nums tracking-tight text-amber-100", "{fmt_hms(p.banked)}" }
            div { class: "text-sm text-foreground", "{p.title}" }
            div { class: "flex gap-2",
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: on_resume,
                    "Resume"
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    on_click: on_discard,
                    "Discard"
                }
            }
        }
    }
}

/// Manual/retro time entry — a collapsible form to log past work (a
/// session you forgot to track, or work with no specific clock time).
/// Fields: description, date, duration in hours, an optional project
/// (which carries the rate), and billable. The clock time is
/// synthesized (date @ 09:00 + duration) so "2 hours on the 14th" is
/// one field, not a start/stop pair — matching how people actually
/// remember past work.
#[component]
fn LogPastForm(slug: String, org_id: Uuid) -> Element {
    let muts = stores::use_timer_mutations();
    let projects = stores::use_project_list();
    let mut open = use_signal(|| false);
    let mut desc = use_signal(String::new);
    let mut date = use_signal(|| Utc::now().date_naive().to_string());
    let mut hours = use_signal(String::new);
    let mut project_id = use_signal(|| None::<Uuid>);
    let mut billable = use_signal(|| true);
    // Select binds a String; parse it into the `project_id` Uuid on change.
    let project_pick = use_signal(String::new);

    let project_rows = projects.value().cloned().unwrap_or_default();
    // Resolve the picked project's vault path for the request.
    let picked_path = {
        let pid = *project_id.read();
        pid.and_then(|id| {
            project_rows
                .iter()
                .map(|(_, p)| p)
                .find(|p| p.project.id == id)
                .map(|p| p.project.path.clone())
        })
        .unwrap_or_default()
    };

    let submit = {
        let slug = slug.clone();
        move |_| {
            let h: f64 = hours.peek().trim().parse().unwrap_or(0.0);
            if h <= 0.0 {
                return;
            }
            let Ok(d) = chrono::NaiveDate::parse_from_str(date.peek().trim(), "%Y-%m-%d") else {
                return;
            };
            let start = Utc.from_utc_datetime(&d.and_hms_opt(9, 0, 0).unwrap());
            let end = start + chrono::Duration::seconds((h * 3600.0) as i64);
            muts.log(
                slug.clone(),
                LogSessionRequest {
                    user_id: owner_id(org_id),
                    org_id,
                    project_id: *project_id.peek(),
                    project_path: picked_path.clone(),
                    task_note_path: String::new(),
                    description: desc.peek().trim().to_string(),
                    start_time: start,
                    end_time: end,
                    billable_override: Some(*billable.peek()),
                },
            );
            desc.set(String::new());
            hours.set(String::new());
        }
    };

    rsx! {
        div { class: "rounded-2xl border border-border/60 bg-card/40",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-2 px-4 py-2.5 text-left text-sm font-medium text-muted-foreground transition-colors hover:text-foreground",
                onclick: move |_| {
                    let v = *open.peek();
                    open.set(!v);
                },
                architect_ui::lucide_dioxus::Plus { size: 14 }
                "Log past time"
            }
            if open() {
                div { class: "flex flex-col gap-3 border-t border-border/60 p-4",
                    input {
                        class: "{FIELD}",
                        r#type: "text",
                        placeholder: "What did you work on?",
                        value: "{desc}",
                        oninput: move |e| desc.set(e.value()),
                    }
                    div { class: "flex flex-col gap-2 sm:flex-row",
                        label { class: "flex flex-1 flex-col gap-1 text-xs text-muted-foreground",
                            "Date"
                            input {
                                class: "{FIELD}",
                                r#type: "date",
                                value: "{date}",
                                oninput: move |e| date.set(e.value()),
                            }
                        }
                        label { class: "flex flex-1 flex-col gap-1 text-xs text-muted-foreground",
                            "Hours"
                            input {
                                class: "{FIELD}",
                                r#type: "number",
                                step: "0.25",
                                min: "0",
                                placeholder: "2.5",
                                value: "{hours}",
                                oninput: move |e| hours.set(e.value()),
                            }
                        }
                    }
                    label { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                        "Project (sets the rate)"
                        Select {
                            value: project_pick,
                            placeholder: "— none —".to_string(),
                            on_change: move |v: String| {
                                project_id.set(Uuid::parse_str(&v).ok());
                            },
                            SelectContent {
                                for (i, (_ , p)) in project_rows.iter().enumerate() {
                                    SelectItem { key: "{p.project.id}", value: "{p.project.id}", index: i, "{p.project.title}" }
                                }
                            }
                        }
                    }
                    div { class: "flex items-center justify-between",
                        Button {
                            variant: if billable() { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                            size: ButtonSize::Small,
                            on_click: move |_| billable.toggle(),
                            if billable() { "Billable ✓" } else { "Non-billable" }
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            on_click: submit,
                            "Log entry"
                        }
                    }
                }
            }
        }
    }
}

/// A task chosen from the picker dropdown — everything the start
/// request needs to link the session to the task's note + project.
#[derive(Clone, PartialEq)]
struct PickedTask {
    title: String,
    /// Vault-relative path of the task's markdown note.
    path: String,
    /// The task's owning project (frontmatter `id:`), if any.
    project_id: Option<Uuid>,
    /// Vault path cache for the project note; empty when unlinked.
    project_path: String,
}

/// One fuzzy-matched dropdown row, with the project chip pre-resolved.
#[derive(Clone, PartialEq)]
struct TaskCandidate {
    id: Uuid,
    title: String,
    path: String,
    project_id: Option<Uuid>,
    project_path: String,
    /// Resolved project title for the `#project` chip, if resolvable.
    project_title: Option<String>,
}

/// Statuses the picker treats as "still workable" — everything except
/// the closed set (mirrors the status vocab in `task_sort` / gantt).
fn is_open_status(status: &str) -> bool {
    !matches!(
        status,
        "done" | "complete" | "completed" | "cancelled" | "shipped"
    )
}

/// The start form: a description input that doubles as a fuzzy task
/// search (dropdown of open tasks), a clearable "for: <task>" chip
/// when one is picked, + Start button. Plain text still starts an
/// unlinked session.
fn start_form(
    mut description: Signal<String>,
    mut picked: Signal<Option<PickedTask>>,
    candidates: Vec<TaskCandidate>,
    on_start: impl FnMut(MouseEvent) + 'static,
) -> Element {
    let pick = picked.read().clone();
    rsx! {
        div { class: "flex flex-col gap-3 rounded-2xl border border-border/70 bg-card/60 p-5",
            Text { variant: TextVariant::Muted, "Nothing tracking right now." }
            if let Some(p) = pick {
                div { class: "flex",
                    span { class: "inline-flex max-w-full items-center gap-1.5 rounded-full border border-primary/40 bg-primary/10 px-2.5 py-0.5 text-xs",
                        span { class: "shrink-0 text-muted-foreground", "for:" }
                        span { class: "truncate text-foreground", "{p.title}" }
                        button {
                            class: "shrink-0 text-muted-foreground transition-colors hover:text-foreground",
                            aria_label: "Clear linked task",
                            onclick: move |_| picked.set(None),
                            "\u{d7}"
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2 sm:flex-row",
                div { class: "relative flex-1",
                    input {
                        class: "w-full rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40",
                        r#type: "text",
                        placeholder: "What are you working on? Type to search tasks",
                        value: "{description}",
                        oninput: move |e| description.set(e.value()),
                    }
                    if !candidates.is_empty() {
                        div {
                            class: "absolute inset-x-0 top-full z-20 mt-1 flex flex-col overflow-hidden rounded-md border border-border bg-popover shadow-lg",
                            for c in candidates {
                                TaskOption {
                                    key: "{c.id}",
                                    candidate: c,
                                    description,
                                    picked,
                                }
                            }
                        }
                    }
                }
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: on_start,
                    "Start"
                }
            }
        }
    }
}

/// One dropdown row: title + `#project` chip. Clicking it fills the
/// description with the task title and records the task link — pure
/// page-signal writes, nothing async.
#[component]
fn TaskOption(
    candidate: TaskCandidate,
    mut description: Signal<String>,
    mut picked: Signal<Option<PickedTask>>,
) -> Element {
    let select = {
        let c = candidate.clone();
        move |_| {
            description.set(c.title.clone());
            picked.set(Some(PickedTask {
                title: c.title.clone(),
                path: c.path.clone(),
                project_id: c.project_id,
                project_path: c.project_path.clone(),
            }));
        }
    };
    rsx! {
        button {
            class: "flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm transition-colors hover:bg-accent",
            r#type: "button",
            onclick: select,
            span { class: "min-w-0 truncate text-foreground", "{candidate.title}" }
            if let Some(project) = candidate.project_title.as_ref() {
                span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[10px] text-muted-foreground",
                    "#{project}"
                }
            }
        }
    }
}

/// Recent sessions, newest first, with a today total. Each row (tagged
/// with its org) edits / deletes in place through the shared store.
fn session_list(rows: &[(Id<Uuid>, stores::OrgSession)]) -> Element {
    let today = Utc::now().date_naive();
    let today_secs: i64 = rows
        .iter()
        .filter(|(_, r)| r.session.start_time.date_naive() == today)
        .filter_map(|(_, r)| {
            r.session
                .end_time
                .map(|e| (e - r.session.start_time).num_seconds())
        })
        .sum();

    rsx! {
        div { class: "flex flex-col gap-2",
            div { class: "flex items-center justify-between",
                Heading { level: HeadingLevel::H3, "Recent" }
                Text { variant: TextVariant::Muted, "Today: {fmt_hms(today_secs)}" }
            }
            div { class: "flex flex-col divide-y divide-border/50 rounded-xl border border-border/60 bg-card/40",
                for (id , row) in rows.iter().take(50) {
                    SessionRow {
                        key: "{id}",
                        pending: id.is_temp(),
                        session: row.session.clone(),
                        slug: row.slug.clone(),
                    }
                }
            }
        }
    }
}

/// One recent-session row — its org badge + an Edit affordance that
/// swaps to an inline editor (description + billable + Save / Cancel /
/// Delete). Editing re-snapshots the rate server-side; the store
/// reconciles against the returned row.
#[component]
fn SessionRow(session: WorkSession, slug: String, pending: bool) -> Element {
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let muts = stores::use_timer_mutations();
    let mut editing = use_signal(|| false);
    // Edit drafts — (re)seeded from the store row when Edit is opened,
    // so a reconciled server value isn't shadowed by a stale draft.
    let mut desc = use_signal(String::new);
    let mut billable = use_signal(|| false);

    let running = session.end_time.is_none();
    let dur = session.end_time.map_or_else(
        || (Utc::now() - session.start_time).num_seconds(),
        |e| (e - session.start_time).num_seconds(),
    );
    let title = if session.description.trim().is_empty() {
        "(no description)".to_string()
    } else {
        session.description.clone()
    };
    let when = session
        .start_time
        .format("%a %b %-d, %-I:%M %p")
        .to_string();
    let id = session.id;
    let stored_desc = session.description.clone();
    let stored_billable = session.billable;

    let org_name = org_list
        .read()
        .iter()
        .find(|o| o.slug == slug)
        .map_or_else(|| slug.clone(), |o| o.name.clone());

    let save = {
        let slug = slug.clone();
        move |_| {
            let req = timer_proto::service::UpdateSessionRequest {
                id,
                description: Some(desc.peek().clone()),
                billable: Some(billable()),
                ..Default::default()
            };
            muts.update(slug.clone(), req);
            editing.set(false);
        }
    };
    let delete = {
        let slug = slug.clone();
        move |_| {
            muts.delete(slug.clone(), id);
        }
    };

    let row_cls = if pending { "opacity-60" } else { "" };

    if editing() {
        rsx! {
            div { class: "flex flex-col gap-2 px-3 py-2.5 {row_cls}",
                input {
                    class: "rounded-md border border-border bg-background px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/40",
                    value: "{desc}",
                    oninput: move |e| desc.set(e.value()),
                }
                div { class: "flex items-center gap-2",
                    Button {
                        variant: if billable() { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                        size: ButtonSize::Small,
                        on_click: move |_| billable.toggle(),
                        if billable() { "Billable ✓" } else { "Non-billable" }
                    }
                    div { class: "flex-1" }
                    Button { variant: ButtonVariant::Ghost, size: ButtonSize::Small, on_click: move |_| editing.set(false), "Cancel" }
                    Button { variant: ButtonVariant::Primary, size: ButtonSize::Small, on_click: save, "Save" }
                    Button { variant: ButtonVariant::Destructive, size: ButtonSize::Small, on_click: delete, "Delete" }
                }
            }
        }
    } else {
        rsx! {
            div { class: "group flex items-center justify-between gap-3 px-3 py-2.5 {row_cls}",
                div { class: "flex min-w-0 flex-col",
                    span { class: "truncate text-sm text-foreground", "{title}" }
                    span { class: "flex items-center gap-1.5 text-xs text-muted-foreground",
                        span { class: "rounded bg-muted px-1.5 py-px text-[10px]", "{org_name}" }
                        span { "{when}" }
                    }
                }
                span {
                    class: if running {
                        "shrink-0 font-mono text-sm tabular-nums text-sky-400"
                    } else {
                        "shrink-0 font-mono text-sm tabular-nums text-muted-foreground"
                    },
                    "{fmt_hms(dur)}"
                }
                button {
                    class: "shrink-0 rounded px-1.5 py-0.5 text-xs text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100",
                    onclick: move |_| {
                        desc.set(stored_desc.clone());
                        billable.set(stored_billable);
                        editing.set(true);
                    },
                    "Edit"
                }
            }
        }
    }
}

// Shared chrome helpers (`resolve_org`, `owner_id`, `fmt_hms`,
// `use_second_tick`) live in `crate::chrome` so the top-bar timer
// widget and this page agree on the same org / owner identity.
