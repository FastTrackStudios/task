//! One row in the list view. Checkbox · title · due pill ·
//! priority badge · context / project chips.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{Ban, Check, CornerDownRight, Ellipsis, Flag, Hash, Trash2, Waypoints};
use uuid::Uuid;

use task_proto::{Priority, Status};

use crate::display::{PriorityLabel, StatusLabel, TaskDisplay};
use crate::{TaskInfo, TaskMutation};

use super::palette::{priority_pill, status_pill};

/// The resolved name of what a task belongs to, for rows whose anchor
/// isn't a project — a parent task, a workstream, a milestone.
///
/// Resolving needs the *other* entity's title, which only the page
/// layer has (it holds the task / workstream / milestone stores), so
/// the row takes the answer rather than the ids. Project-anchored
/// rows don't need this: their chip already comes from `projects`.
#[derive(Clone, PartialEq)]
pub struct AnchorChip {
    /// `"parent"` | `"workstream"` | `"milestone"` — drives the icon.
    pub kind: String,
    /// Display name of the thing this task belongs to.
    pub label: String,
}

#[derive(Props, Clone, PartialEq)]
pub struct TaskRowProps {
    pub task: TaskInfo,
    /// What this task belongs to when it isn't a project (see
    /// [`AnchorChip`]). `None` for project-anchored rows — and, now
    /// that unfiled tasks route to triage instead of the list, never
    /// `None` alongside an empty `projects`.
    #[props(default)]
    pub anchor: Option<AnchorChip>,
    pub on_toggle: EventHandler<Uuid>,
    pub on_open: EventHandler<Uuid>,
    /// Mutation sink for the row's overflow menu (complete / cancel /
    /// delete). Same channel the detail panel and quick-add use.
    pub on_event: EventHandler<TaskMutation>,
}

#[component]
pub fn TaskRow(props: TaskRowProps) -> Element {
    let t = props.task.clone();
    let id = t.id;
    let done = t.is_done();
    let status = t.status_enum();
    let priority = t.priority_enum();

    let title_cls = if done {
        "text-sm text-muted-foreground line-through truncate"
    } else {
        "text-sm text-foreground truncate"
    };

    let tracked = t.tracked_seconds(chrono::Utc::now());
    // Identity chips (what/where kind of task) sit NEXT TO the title;
    // temporal chips (due, tracked time) go to the right edge. Both
    // ride the title line from `sm:` up and wrap below it on touch
    // widths — no orphaned pills across a wide row.
    let anchor = props.anchor.clone();
    let identity = {
        let any = !t.contexts.is_empty()
            || !t.projects.is_empty()
            || anchor.is_some()
            || priority != Priority::Normal
            || (status != Status::Open && status != Status::Done && status != Status::InProgress);
        if any {
            rsx! {
                if priority != Priority::Normal {
                    span {
                        class: "inline-flex items-center rounded-full px-1.5 py-0 text-[10px] uppercase tracking-wider {priority_pill(priority)}",
                        "{priority.label()}"
                    }
                }
                if status != Status::Open && status != Status::Done && status != Status::InProgress {
                    span {
                        class: "inline-flex items-center rounded-full px-1.5 py-0 text-[10px] uppercase tracking-wider {status_pill(status)}",
                        "{status.label()}"
                    }
                }
                for c in t.contexts.iter() {
                    span {
                        key: "{c}",
                        class: "inline-flex items-center gap-0.5 rounded-full bg-muted/50 px-1.5 py-0 text-[10px] text-muted-foreground",
                        "@{c}"
                    }
                }
                for p in t.projects.iter() {
                    span {
                        key: "{p}",
                        class: "inline-flex items-center gap-0.5 rounded-full bg-violet-900/30 px-1.5 py-0 text-[10px] text-violet-200",
                        Hash { size: 9 }
                        "{strip_wikilink(p)}"
                    }
                }
                // Non-project belonging — the chip that keeps a row
                // from reading as a bare title.
                if let Some(a) = anchor.clone() {
                    span {
                        class: "inline-flex items-center gap-0.5 rounded-full bg-slate-700/40 px-1.5 py-0 text-[10px] text-slate-200",
                        title: "{anchor_tooltip(&a.kind)}",
                        match a.kind.as_str() {
                            "workstream" => rsx! { Waypoints { size: 9 } },
                            "milestone" => rsx! { Flag { size: 9 } },
                            _ => rsx! { CornerDownRight { size: 9 } },
                        }
                        "{a.label}"
                    }
                }
            }
        } else {
            rsx! {}
        }
    };
    let temporal = {
        if t.due.is_some() || tracked > 0 {
            rsx! {
                if tracked > 0 {
                    // The task's time receipt — sky while the clock
                    // runs, muted once it's banked.
                    span {
                        class: if status == Status::InProgress {
                            "inline-flex items-center rounded-full bg-sky-500/15 px-1.5 py-0 text-[10px] tabular-nums text-sky-400"
                        } else {
                            "inline-flex items-center rounded-full bg-muted/50 px-1.5 py-0 text-[10px] tabular-nums text-muted-foreground"
                        },
                        {crate::display::duration_label(tracked)}
                    }
                }
                if let Some(d) = t.due_date() {
                    {
                        let (label, cls) = due_pill(d, done);
                        rsx! {
                            span { class: "{cls}", "{label}" }
                        }
                    }
                }
            }
        } else {
            rsx! {}
        }
    };

    rsx! {
        div {
            class: "group flex min-h-[44px] items-center gap-2.5 rounded-md border border-transparent px-2 py-2 hover:border-border hover:bg-accent/30 cursor-pointer sm:min-h-0 sm:py-1.5",
            onclick: move |_| props.on_open.call(id),
            CheckboxButton {
                status,
                priority,
                on_click: move |()| props.on_toggle.call(id),
            }
            div { class: "flex-1 min-w-0 flex flex-col gap-0.5",
                div { class: "flex items-center gap-2 min-w-0",
                    span { class: "{title_cls}", "{t.title}" }
                    div { class: "hidden shrink-0 items-center gap-1.5 sm:flex",
                        {identity.clone()}
                    }
                    div { class: "ml-auto hidden shrink-0 items-center gap-1.5 sm:flex",
                        {temporal.clone()}
                    }
                }
                // Touch widths: everything wraps below the title.
                div { class: "flex items-center gap-1.5 flex-wrap sm:hidden empty:hidden",
                    {identity}
                    {temporal}
                }
            }
            RowMenu { id, done, on_event: props.on_event }
        }
    }
}

/// Per-row overflow menu (⋯) — complete/reopen, cancel (hide without
/// deleting), and a two-click delete. Hidden until hover on desktop,
/// always visible on touch. Stops click propagation so acting on a row
/// never also opens its detail panel.
#[component]
fn RowMenu(id: Uuid, done: bool, on_event: EventHandler<TaskMutation>) -> Element {
    let mut open = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    rsx! {
        div { class: "relative shrink-0",
            button {
                r#type: "button",
                class: "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground opacity-100 transition-colors hover:bg-accent hover:text-foreground sm:opacity-0 sm:group-hover:opacity-100",
                title: "Task actions",
                onclick: move |e: MouseEvent| {
                    e.stop_propagation();
                    let v = *open.peek();
                    open.set(!v);
                    confirm_delete.set(false);
                },
                Ellipsis { size: 16 }
            }
            if open() {
                // Click-away backdrop.
                div {
                    class: "fixed inset-0 z-30",
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        open.set(false);
                        confirm_delete.set(false);
                    },
                }
                div {
                    class: "absolute right-0 top-full z-40 mt-1 flex w-44 flex-col overflow-hidden rounded-lg border border-border bg-popover py-1 text-xs shadow-xl",
                    onclick: move |e: MouseEvent| e.stop_propagation(),
                    button {
                        r#type: "button",
                        class: "flex items-center gap-2 px-3 py-1.5 text-left hover:bg-accent",
                        onclick: move |_| {
                            on_event.call(TaskMutation::SetStatus {
                                id,
                                status: if done { "open".into() } else { "done".into() },
                            });
                            open.set(false);
                        },
                        Check { size: 13 }
                        if done { "Reopen" } else { "Complete" }
                    }
                    button {
                        r#type: "button",
                        class: "flex items-center gap-2 px-3 py-1.5 text-left hover:bg-accent",
                        onclick: move |_| {
                            on_event.call(TaskMutation::SetStatus { id, status: "cancelled".into() });
                            open.set(false);
                        },
                        Ban { size: 13 }
                        "Cancel (hide)"
                    }
                    div { class: "my-1 h-px bg-border/60" }
                    button {
                        r#type: "button",
                        class: "flex items-center gap-2 px-3 py-1.5 text-left text-destructive hover:bg-destructive/10",
                        onclick: move |_| {
                            if *confirm_delete.peek() {
                                on_event.call(TaskMutation::Delete { id });
                                open.set(false);
                                confirm_delete.set(false);
                            } else {
                                confirm_delete.set(true);
                            }
                        },
                        Trash2 { size: 13 }
                        if confirm_delete() { "Confirm delete" } else { "Delete" }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct CheckboxButtonProps {
    status: Status,
    priority: Priority,
    on_click: EventHandler<()>,
}

/// Three-state checkbox: empty (open) → pulsing ring (in-progress,
/// the timer is running) → filled check (done). Clicks walk the
/// domain's `click_transition` cycle via `on_toggle`.
#[component]
pub fn CheckboxButton(props: CheckboxButtonProps) -> Element {
    let base =
        "flex h-6 w-6 sm:h-5 sm:w-5 items-center justify-center rounded-md border-2 shrink-0";
    let (cls, glyph) = match props.status {
        Status::Done | Status::Cancelled => (
            format!("{base} bg-emerald-500 border-emerald-500 text-white"),
            Some(rsx! { Check { size: 12 } }),
        ),
        Status::InProgress => (
            format!("{base} border-sky-500 text-sky-500 hover:bg-sky-500/10"),
            // Running-timer dot — pulses so the active task reads at
            // a glance.
            Some(rsx! { span { class: "h-2 w-2 animate-pulse rounded-full bg-sky-500" } }),
        ),
        _ => {
            let edge = match props.priority {
                Priority::Critical => "border-rose-500",
                Priority::High => "border-amber-500",
                _ => "border-border",
            };
            (
                format!("{base} bg-transparent hover:bg-accent/30 {edge}"),
                None,
            )
        }
    };
    rsx! {
        button {
            r#type: "button",
            class: "{cls}",
            title: if props.status == Status::InProgress { "Tracking — click to complete" } else { "" },
            onclick: move |e: MouseEvent| {
                e.stop_propagation();
                props.on_click.call(());
            },
            {glyph}
        }
    }
}

/// Hover text spelling out the relationship the icon compresses.
fn anchor_tooltip(kind: &str) -> &'static str {
    match kind {
        "workstream" => "Workstream",
        "milestone" => "Milestone",
        _ => "Subtask of",
    }
}

/// Convert a `[[Wikilink]]` into the bare page name.
pub(super) fn strip_wikilink(s: &str) -> String {
    s.trim_start_matches("[[")
        .trim_end_matches("]]")
        .to_string()
}

/// Coloring for the due-date pill — red when overdue, amber for
/// today, sky for upcoming, muted once done.
fn due_pill(d: chrono::NaiveDate, done: bool) -> (String, &'static str) {
    let today = chrono::Local::now().date_naive();
    let label = if d == today {
        "Today".to_string()
    } else if d == today + chrono::Duration::days(1) {
        "Tomorrow".to_string()
    } else if d == today - chrono::Duration::days(1) {
        "Yesterday".to_string()
    } else {
        d.format("%b %-d").to_string()
    };
    let cls = if done {
        "inline-flex items-center rounded-full px-1.5 py-0 text-[10px] text-muted-foreground border border-border"
    } else if d < today {
        "inline-flex items-center rounded-full px-1.5 py-0 text-[10px] text-rose-100 bg-rose-700/50 border border-rose-500"
    } else if d == today {
        "inline-flex items-center rounded-full px-1.5 py-0 text-[10px] text-amber-50 bg-amber-700/60 border border-amber-400 font-medium"
    } else if d <= today + chrono::Duration::days(7) {
        "inline-flex items-center rounded-full px-1.5 py-0 text-[10px] text-sky-100 bg-sky-700/40 border border-sky-500"
    } else {
        "inline-flex items-center rounded-full px-1.5 py-0 text-[10px] text-muted-foreground border border-border"
    };
    (label, cls)
}
