//! Right-side detail sheet — the quick-edit path for `/tasks`.
//!
//! Everything saves as it changes, the way a phone app does: taps
//! (status, priority, a date, the project, the owner, start/stop) at
//! once; typed fields (title, notes, a typed date) when you leave
//! them. A "Saved" mark confirms it. There is no Save/Cancel pair,
//! because half the fields saving on tap and half waiting for a button
//! made Cancel a lie.
//!
//! Delete sits behind the ⋯ menu with a confirm step, away from the
//! close button; the page offers Undo afterwards.
//!
//! Built on architect-ui's `Sheet` (overlay + escape-to-close + close
//! button come from the primitive). The richer read surface lives in
//! [`super::detail_full::TaskDetailFull`].

use architect_ui::lucide_dioxus::{Check, Ellipsis, Play, Square, Trash2};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::TaskInfo;
use crate::TaskMutation;
use task_proto::{Priority, Status};

use crate::display::{StatusLabel, TaskDisplay};

#[derive(Props, Clone, PartialEq)]
pub struct TaskDetailProps {
    pub task: TaskInfo,
    pub on_event: EventHandler<TaskMutation>,
    pub on_close: EventHandler<()>,
    /// When set, the header offers "Open full view" emitting the
    /// task id — the page layer routes it to `/tasks/:id`.
    #[props(default)]
    pub on_open_full: Option<EventHandler<Uuid>>,
    /// `(id, title)` of every project the task can be filed under.
    /// Empty ⇒ no project picker.
    #[props(default)]
    pub projects: Vec<(Uuid, String)>,
    /// `(user id, name)` of the people the task can be given to.
    /// Empty ⇒ no owner picker.
    #[props(default)]
    pub people: Vec<(String, String)>,
}

/// One-tap due dates — rescheduling is the commonest edit a task list
/// sees, and a native date picker is three taps and a scroll on a phone.
fn due_shortcuts(today: chrono::NaiveDate) -> [(&'static str, Option<chrono::NaiveDate>); 4] {
    use chrono::Datelike;
    let days_to_monday = 7 - today.weekday().num_days_from_monday();
    [
        ("Today", Some(today)),
        ("Tomorrow", today.succ_opt()),
        (
            "Next week",
            Some(today + chrono::Duration::days(i64::from(days_to_monday))),
        ),
        ("No date", None),
    ]
}

/// The four priorities a person picks between. `None` and `Normal`
/// were two words for "nothing special"; the sheet shows one.
const PRIORITIES: [(Priority, &str); 4] = [
    (Priority::Low, "Low"),
    (Priority::Normal, "Normal"),
    (Priority::High, "High"),
    (Priority::Critical, "Urgent"),
];

/// The first assignee's user id, if a human holds the task.
fn owner_id(t: &TaskInfo) -> Option<String> {
    use workflows_proto::AgentRef;
    match t.workflow.as_ref()?.assignees.0.first()? {
        AgentRef::Human { user_id, .. } => Some(user_id.clone()),
        AgentRef::Agent { .. } => None,
    }
}

const CHOICE_ON: &str = "rounded-md border border-primary bg-primary/15 px-3 py-2 text-sm font-medium text-foreground sm:px-2.5 sm:py-1 sm:text-xs";
const CHOICE_OFF: &str = "rounded-md border border-border px-3 py-2 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:px-2.5 sm:py-1 sm:text-xs";
const FIELD: &str = "rounded-md border border-border bg-card px-2 py-1.5 text-base text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring sm:text-sm";

#[component]
pub fn TaskDetail(props: TaskDetailProps) -> Element {
    let initial = props.task.clone();
    let mut title = use_signal(|| initial.title.clone());
    let mut due = use_signal(|| initial.due.clone().unwrap_or_default());
    let mut details = use_signal(|| initial.details.clone());
    let mut project = use_signal(|| initial.project_id);
    let mut owner = use_signal(|| owner_id(&initial));
    let mut saved = use_signal(|| false);
    let mut menu = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let id = initial.id;
    let current_status = initial.status_enum();
    let current_priority = match initial.priority_enum() {
        Priority::None => Priority::Normal,
        p => p,
    };

    // The task as the sheet currently shows it — what every save writes.
    let draft = {
        let initial = initial.clone();
        let people = props.people.clone();
        move || {
            let mut next = initial.clone();
            next.title = title.read().clone();
            let d = due.read().clone();
            next.due = if d.is_empty() { None } else { Some(d) };
            next.details = details.read().clone();
            let chosen = *project.read();
            if chosen != initial.project_id {
                next.project_id = chosen;
                // A `[[Project]]` wikilink would contradict the id.
                next.projects = Default::default();
            }
            let who = owner.read().clone();
            if who != owner_id(&initial) {
                let wf = next.workflow.get_or_insert_with(Default::default);
                wf.assignees.0 = who
                    .map(|user_id| {
                        let display_name = people
                            .iter()
                            .find(|(uid, _)| *uid == user_id)
                            .map(|(_, n)| n.clone());
                        vec![workflows_proto::AgentRef::Human {
                            user_id,
                            display_name,
                        }]
                    })
                    .unwrap_or_default();
            }
            next
        }
    };
    let commit = {
        let draft = draft.clone();
        move || {
            props.on_event.call(TaskMutation::Update { task: draft() });
            // A local copy keeps this closure `Fn` (signals are `Copy`).
            let mut mark = saved;
            mark.set(true);
        }
    };
    // Typed fields save when you leave them, and only if they changed.
    let commit_if_changed = {
        let commit = commit.clone();
        let initial = initial.clone();
        move || {
            let changed = *title.read() != initial.title
                || *details.read() != initial.details
                || *due.read() != initial.due.clone().unwrap_or_default();
            if changed {
                commit();
            }
        }
    };
    let today = chrono::Local::now().date_naive();
    let working = current_status == Status::InProgress;
    // A project the task points at that this board can't name (another
    // org's, in All mode) is not "No project".
    let unknown_project = initial
        .project_id
        .filter(|pid| !props.projects.iter().any(|(id, _)| id == pid));

    rsx! {
        Sheet {
            open: true,
            side: SheetSide::Right,
            // The primitive caps at sm:max-w-sm; this sheet has
            // always been 28rem on desktop, full-width on phones.
            class: "w-full max-w-[28rem] sm:max-w-[28rem] gap-4 overflow-y-auto p-4",
            on_close: move |()| props.on_close.call(()),
            div { class: "flex items-center gap-2 pr-8",
                if saved() {
                    span { class: "inline-flex items-center gap-1 text-xs text-muted-foreground", role: "status",
                        Check { size: 13 }
                        "Saved"
                    }
                }
                div { class: "ml-auto flex items-center gap-1",
                    if let Some(open_full) = props.on_open_full {
                        button {
                            r#type: "button",
                            class: "inline-flex items-center gap-1 rounded-md px-2 py-1.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                            onclick: move |_| {
                                open_full.call(id);
                                props.on_close.call(());
                            },
                            "Full view"
                        }
                    }
                    div { class: "relative",
                        button {
                            r#type: "button",
                            class: "inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                            aria_label: "More actions",
                            aria_haspopup: "menu",
                            aria_expanded: menu().to_string(),
                            onclick: move |_| {
                                let v = *menu.peek();
                                menu.set(!v);
                                confirm_delete.set(false);
                            },
                            Ellipsis { size: 16 }
                        }
                        if menu() {
                            div {
                                class: "absolute right-0 top-full z-50 mt-1 flex w-44 flex-col overflow-hidden rounded-lg border border-border bg-popover py-1 text-sm shadow-xl",
                                role: "menu",
                                button {
                                    r#type: "button",
                                    role: "menuitem",
                                    class: "flex items-center gap-2 px-3 py-2 text-left text-destructive hover:bg-destructive/10",
                                    onclick: move |_| {
                                        if *confirm_delete.peek() {
                                            props.on_event.call(TaskMutation::Delete { id });
                                            props.on_close.call(());
                                        } else {
                                            confirm_delete.set(true);
                                        }
                                    },
                                    Trash2 { size: 14 }
                                    if confirm_delete() { "Confirm delete" } else { "Delete task" }
                                }
                            }
                        }
                    }
                }
            }
            input {
                r#type: "text",
                aria_label: "Title",
                class: "w-full rounded-sm border-b border-border bg-transparent py-1 text-lg font-medium text-foreground outline-none focus-visible:border-primary",
                value: "{title}",
                oninput: move |e| title.set(e.value()),
                onblur: {
                    let save = commit_if_changed.clone();
                    move |_| save()
                },
            }
            // Start / stop work — the clock, kept apart from completion.
            if !matches!(current_status, Status::Done | Status::Cancelled) {
                button {
                    r#type: "button",
                    class: if working {
                        "inline-flex min-h-11 items-center justify-center gap-2 rounded-md bg-sky-500/15 px-3 text-sm font-medium text-foreground hover:bg-sky-500/25 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:min-h-9"
                    } else {
                        "inline-flex min-h-11 items-center justify-center gap-2 rounded-md border border-border px-3 text-sm font-medium text-foreground hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:min-h-9"
                    },
                    onclick: move |_| {
                        props.on_event.call(TaskMutation::SetStatus {
                            id,
                            status: task_proto::track_transition(&initial.status).to_string(),
                        });
                        saved.set(true);
                    },
                    if working { Square { size: 14 } } else { Play { size: 14 } }
                    if working { "Stop working" } else { "Start working" }
                }
            }
            div { class: "flex flex-col gap-2",
                FieldLabel { label: "Status" }
                div { class: "flex flex-wrap gap-1",
                    for s in [Status::Open, Status::InProgress, Status::Waiting, Status::Done] {
                        button {
                            key: "{s.as_str()}",
                            r#type: "button",
                            class: if s == current_status { CHOICE_ON } else { CHOICE_OFF },
                            aria_pressed: (s == current_status).to_string(),
                            onclick: move |_| {
                                props.on_event.call(TaskMutation::SetStatus { id, status: s.as_str().to_string() });
                                saved.set(true);
                            },
                            "{s.label()}"
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2",
                FieldLabel { label: "Priority" }
                div { class: "flex flex-wrap gap-1",
                    for (p, label) in PRIORITIES {
                        button {
                            key: "{p.as_str()}",
                            r#type: "button",
                            class: if p == current_priority { CHOICE_ON } else { CHOICE_OFF },
                            aria_pressed: (p == current_priority).to_string(),
                            onclick: move |_| {
                                props.on_event.call(TaskMutation::SetPriority { id, priority: p.as_str().to_string() });
                                saved.set(true);
                            },
                            "{label}"
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2",
                FieldLabel { label: "Due" }
                div { class: "flex flex-wrap gap-1",
                    for (label, day) in due_shortcuts(today) {
                        {
                            let value = day.map(|d| d.to_string()).unwrap_or_default();
                            let active = *due.read() == value;
                            let commit = commit.clone();
                            rsx! {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    class: if active { CHOICE_ON } else { CHOICE_OFF },
                                    aria_pressed: active.to_string(),
                                    onclick: move |_| {
                                        due.set(value.clone());
                                        commit();
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
                input {
                    r#type: "date",
                    aria_label: "Due date",
                    class: "{FIELD}",
                    value: "{due}",
                    onchange: {
                        let commit = commit.clone();
                        move |e: Event<FormData>| {
                            due.set(e.value());
                            commit();
                        }
                    },
                }
            }
            if !props.projects.is_empty() || unknown_project.is_some() {
                div { class: "flex flex-col gap-2",
                    FieldLabel { label: "Project" }
                    select {
                        aria_label: "Project",
                        class: "{FIELD}",
                        onchange: {
                            let commit = commit.clone();
                            move |e: Event<FormData>| {
                                project.set(Uuid::parse_str(&e.value()).ok());
                                commit();
                            }
                        },
                        option { value: "", selected: project.read().is_none(), "No project" }
                        if let Some(pid) = unknown_project {
                            option { value: "{pid}", selected: *project.read() == Some(pid), "A project not in this view" }
                        }
                        for (pid, name) in props.projects.iter() {
                            option {
                                key: "{pid}",
                                value: "{pid}",
                                selected: *project.read() == Some(*pid),
                                "{name}"
                            }
                        }
                    }
                }
            }
            if !props.people.is_empty() {
                div { class: "flex flex-col gap-2",
                    FieldLabel { label: "Owner" }
                    select {
                        aria_label: "Owner",
                        class: "{FIELD}",
                        onchange: {
                            let commit = commit.clone();
                            move |e: Event<FormData>| {
                                let v = e.value();
                                owner.set((!v.is_empty()).then_some(v));
                                commit();
                            }
                        },
                        option { value: "", selected: owner.read().is_none(), "Nobody yet" }
                        for (uid, name) in props.people.iter() {
                            option {
                                key: "{uid}",
                                value: "{uid}",
                                selected: owner.read().as_deref() == Some(uid.as_str()),
                                "{name}"
                            }
                        }
                    }
                }
            }
            if !initial.contexts.is_empty() {
                div { class: "flex flex-col gap-2",
                    FieldLabel { label: "Contexts" }
                    div { class: "flex flex-wrap gap-1",
                        for c in initial.contexts.iter() {
                            span {
                                key: "{c}",
                                class: "rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground",
                                "@{c.trim_start_matches('@')}"
                            }
                        }
                    }
                }
            }
            div { class: "flex min-h-0 flex-1 flex-col gap-2",
                FieldLabel { label: "Notes" }
                textarea {
                    aria_label: "Notes",
                    class: "min-h-[8rem] flex-1 resize-none {FIELD}",
                    placeholder: "Add notes",
                    value: "{details}",
                    oninput: move |e| details.set(e.value()),
                    onblur: {
                        let save = commit_if_changed.clone();
                        move |_| save()
                    },
                }
            }
        }
    }
}

#[component]
fn FieldLabel(label: &'static str) -> Element {
    rsx! {
        span { class: "text-xs font-medium text-muted-foreground", "{label}" }
    }
}

#[cfg(test)]
mod tests {
    use super::due_shortcuts;

    fn day(s: &str) -> chrono::NaiveDate {
        s.parse().unwrap()
    }

    /// "Next week" is the coming Monday — from a Tuesday, and from a
    /// Sunday (tomorrow), never today.
    #[test]
    fn next_week_is_the_coming_monday() {
        let from_tuesday = due_shortcuts(day("2026-10-06"));
        assert_eq!(from_tuesday[0].1, Some(day("2026-10-06")));
        assert_eq!(from_tuesday[1].1, Some(day("2026-10-07")));
        assert_eq!(from_tuesday[2].1, Some(day("2026-10-12")));
        assert_eq!(from_tuesday[3].1, None);

        assert_eq!(
            due_shortcuts(day("2026-10-11"))[2].1,
            Some(day("2026-10-12"))
        );
        assert_eq!(
            due_shortcuts(day("2026-10-12"))[2].1,
            Some(day("2026-10-19"))
        );
    }
}
