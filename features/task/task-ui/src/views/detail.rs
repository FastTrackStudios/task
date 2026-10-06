//! Right-side detail sheet — edit title, status, priority,
//! due date. Save emits an `Update` mutation; per-field
//! checkbox / status flips emit the cheaper `SetStatus` /
//! `SetPriority` paths.
//!
//! Built on architect-ui's `Sheet` (overlay + escape-to-close + close
//! button come from the primitive). The richer read surface
//! (markdown body, workflow, subtasks, session history) lives
//! in [`super::detail_full::TaskDetailFull`] — this sheet stays
//! the quick-edit path for `/tasks` today.

use architect_ui::lucide_dioxus::Trash2;
use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::TaskInfo;
use crate::TaskMutation;
use task_proto::{Priority, Status};

use crate::display::{PriorityLabel, StatusLabel, TaskDisplay};

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

#[component]
pub fn TaskDetail(props: TaskDetailProps) -> Element {
    let initial = props.task.clone();
    let mut title = use_signal(|| initial.title.clone());
    let mut due = use_signal(|| initial.due.clone().unwrap_or_default());
    let mut details = use_signal(|| initial.details.clone());
    let mut project = use_signal(|| initial.project_id);
    let id = initial.id;
    let current_status = initial.status_enum();
    let current_priority = initial.priority_enum();

    // The task as the sheet currently shows it. Date shortcuts and the
    // project picker save at once — like status and priority — so a
    // reschedule is one tap, carrying whatever else was typed along.
    let draft = {
        let initial = initial.clone();
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
            next
        }
    };
    let commit = {
        let draft = draft.clone();
        move || props.on_event.call(TaskMutation::Update { task: draft() })
    };
    let today = chrono::Local::now().date_naive();

    rsx! {
        Sheet {
            open: true,
            side: SheetSide::Right,
            // The primitive caps at sm:max-w-sm; this sheet has
            // always been 28rem on desktop, full-width on phones.
            class: "w-full max-w-[28rem] sm:max-w-[28rem] gap-3 overflow-y-auto p-4",
            on_close: move |()| props.on_close.call(()),
            div { class: "flex items-center justify-between pr-8",
                span { class: "text-xs uppercase tracking-wider text-muted-foreground", "Task" }
                if let Some(open_full) = props.on_open_full {
                    button {
                        r#type: "button",
                        class: "inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                        title: "Open full view",
                        onclick: move |_| {
                            open_full.call(id);
                            props.on_close.call(());
                        },
                        "Full view"
                    }
                }
                button {
                    r#type: "button",
                    class: "inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                    title: "Delete",
                    onclick: move |_| {
                        props.on_event.call(TaskMutation::Delete { id });
                        props.on_close.call(());
                    },
                    Trash2 { size: 14 }
                }
            }
            input {
                r#type: "text",
                class: "w-full bg-transparent text-lg font-medium text-foreground outline-none border-b border-border focus:border-primary py-1",
                value: "{title}",
                oninput: move |e| title.set(e.value()),
            }
            div { class: "flex flex-col gap-2",
                FieldLabel { label: "Status" }
                div { class: "flex flex-wrap gap-1",
                    for s in [Status::Open, Status::InProgress, Status::Waiting, Status::Done, Status::Cancelled] {
                        {
                            let active = s == current_status;
                            let cls = if active {
                                "rounded-md border border-primary bg-primary/15 text-foreground px-3 py-2 sm:px-2.5 sm:py-1 text-xs font-medium"
                            } else {
                                "rounded-md border border-border text-muted-foreground hover:text-foreground hover:bg-accent px-3 py-2 sm:px-2.5 sm:py-1 text-xs"
                            };
                            rsx! {
                                button {
                                    key: "{s.as_str()}",
                                    r#type: "button",
                                    class: "{cls}",
                                    onclick: move |_| props.on_event.call(TaskMutation::SetStatus { id, status: s.as_str().to_string() }),
                                    "{s.label()}"
                                }
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2",
                FieldLabel { label: "Priority" }
                div { class: "flex flex-wrap gap-1",
                    for p in [Priority::None, Priority::Low, Priority::Normal, Priority::High, Priority::Critical] {
                        {
                            let active = p == current_priority;
                            let cls = if active {
                                "rounded-md border border-primary bg-primary/15 text-foreground px-3 py-2 sm:px-2.5 sm:py-1 text-xs font-medium"
                            } else {
                                "rounded-md border border-border text-muted-foreground hover:text-foreground hover:bg-accent px-3 py-2 sm:px-2.5 sm:py-1 text-xs"
                            };
                            rsx! {
                                button {
                                    key: "{p.as_str()}",
                                    r#type: "button",
                                    class: "{cls}",
                                    onclick: move |_| props.on_event.call(TaskMutation::SetPriority { id, priority: p.as_str().to_string() }),
                                    "{p.label()}"
                                }
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2",
                FieldLabel { label: "Due date" }
                div { class: "flex flex-wrap gap-1",
                    for (label, day) in due_shortcuts(today) {
                        {
                            let value = day.map(|d| d.to_string()).unwrap_or_default();
                            let active = *due.read() == value;
                            let cls = if active {
                                "rounded-md border border-primary bg-primary/15 text-foreground px-3 py-2 sm:px-2.5 sm:py-1 text-xs font-medium"
                            } else {
                                "rounded-md border border-border text-muted-foreground hover:text-foreground hover:bg-accent px-3 py-2 sm:px-2.5 sm:py-1 text-xs"
                            };
                            let commit = commit.clone();
                            rsx! {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    class: "{cls}",
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
                    class: "rounded-md border border-border bg-card px-2 py-1.5 text-base text-foreground outline-none focus:border-primary sm:text-sm",
                    value: "{due}",
                    oninput: move |e| due.set(e.value()),
                }
            }
            if !props.projects.is_empty() {
                div { class: "flex flex-col gap-2",
                    FieldLabel { label: "Project" }
                    select {
                        class: "rounded-md border border-border bg-card px-2 py-1.5 text-base text-foreground outline-none focus:border-primary sm:text-sm",
                        onchange: {
                            let commit = commit.clone();
                            move |e: Event<FormData>| {
                                project.set(Uuid::parse_str(&e.value()).ok());
                                commit();
                            }
                        },
                        option { value: "", selected: project.read().is_none(), "No project" }
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
            if !initial.contexts.is_empty() || !initial.projects.is_empty() {
                div { class: "flex flex-col gap-2",
                    FieldLabel { label: "Tags" }
                    div { class: "flex flex-wrap gap-1",
                        for c in initial.contexts.iter() {
                            span {
                                key: "{c}",
                                class: "rounded-full bg-muted/50 px-2 py-0.5 text-[11px] text-muted-foreground",
                                "@{c}"
                            }
                        }
                        for p in initial.projects.iter() {
                            span {
                                key: "{p}",
                                class: "rounded-full bg-violet-900/30 px-2 py-0.5 text-[11px] text-violet-200",
                                "{p.trim_start_matches(\"[[\").trim_end_matches(\"]]\")}"
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2 flex-1 min-h-0",
                FieldLabel { label: "Notes" }
                textarea {
                    class: "flex-1 min-h-[8rem] resize-none rounded-md border border-border bg-card px-2 py-1.5 text-sm text-foreground outline-none focus:border-primary",
                    placeholder: "Notes…",
                    value: "{details}",
                    oninput: move |e| details.set(e.value()),
                }
            }
            SheetFooter { class: "border-t border-border pt-2 gap-2",
                // Full-width, touch-sized on phones; compact on desktop.
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    class: "flex-1 min-h-11 sm:flex-none sm:min-h-0",
                    on_click: move |_| props.on_close.call(()),
                    "Cancel"
                }
                Button {
                    size: ButtonSize::Small,
                    class: "flex-1 min-h-11 sm:flex-none sm:min-h-0",
                    on_click: move |_| {
                        props.on_event.call(TaskMutation::Update { task: draft() });
                        props.on_close.call(());
                    },
                    "Save"
                }
            }
        }
    }
}

#[component]
fn FieldLabel(label: &'static str) -> Element {
    rsx! {
        span { class: "text-[10px] uppercase tracking-wider text-muted-foreground", "{label}" }
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
