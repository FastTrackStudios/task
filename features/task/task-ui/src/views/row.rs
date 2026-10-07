//! One row in the list view: checkbox · title · how late · what it
//! belongs to · who owns it · when it's due · start/stop · ⋯.
//!
//! Read at a glance (a dim studio, a two-second phone check): the
//! title, and — when it's late — how late, beside it rather than at the
//! far edge. Everything else is smaller and quieter.

use architect_ui::lucide_dioxus::{
    Ban, Check, CornerDownRight, Ellipsis, Flag, Hash, Play, Square, Trash2, Waypoints,
};
use dioxus::prelude::*;
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
    /// [`AnchorChip`]).
    #[props(default)]
    pub anchor: Option<AnchorChip>,
    /// The org the task lives in, shown only when the list spans
    /// several (All mode) — a team's tasks and a client's look alike
    /// otherwise.
    #[props(default)]
    pub org: Option<String>,
    /// Complete / reopen (the checkbox).
    pub on_toggle: EventHandler<Uuid>,
    pub on_open: EventHandler<Uuid>,
    /// Start / stop work (the ▶ control and the menu item).
    pub on_track: EventHandler<Uuid>,
    /// Mutation sink for the row's overflow menu (won't do / delete).
    /// Same channel the detail panel and quick-add use.
    pub on_event: EventHandler<TaskMutation>,
}

/// Whose task this is: the first assignee, by name when the record
/// carries one.
#[must_use]
pub fn owner_name(t: &TaskInfo) -> Option<String> {
    use workflows_proto::AgentRef;
    let first = t.workflow.as_ref()?.assignees.0.first()?;
    Some(match first {
        AgentRef::Human {
            user_id,
            display_name,
        } => display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| user_id.chars().take(8).collect()),
        AgentRef::Agent { name, .. } => name.clone(),
    })
}

/// Two-letter initials for an avatar.
fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().and_then(|w| w.chars().next());
    let second = words.last().and_then(|w| w.chars().next());
    match (first, second) {
        (Some(a), Some(b)) => format!("{a}{b}").to_uppercase(),
        (Some(_), None) => name.chars().take(2).collect::<String>().to_uppercase(),
        _ => "?".to_owned(),
    }
}

/// Whole days a task is past its due date, `None` when it isn't.
#[must_use]
pub fn days_late(t: &TaskInfo, today: chrono::NaiveDate) -> Option<i64> {
    if t.is_done() {
        return None;
    }
    let due = t.due_date()?;
    (due < today).then(|| (today - due).num_days())
}

/// A clock is actually running on this task: an open time entry, not
/// merely an `in-progress` status.
#[must_use]
pub fn clock_running(t: &TaskInfo) -> bool {
    t.time_entries.iter().any(|e| e.end_time.is_none())
}

const CHIP: &str = "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs leading-none";

#[component]
pub fn TaskRow(props: TaskRowProps) -> Element {
    let t = props.task.clone();
    let id = t.id;
    let done = t.is_done();
    let status = t.status_enum();
    let priority = t.priority_enum();
    let live = clock_running(&t);
    let today = chrono::Local::now().date_naive();
    let late = days_late(&t, today);
    let owner = owner_name(&t);

    let title_cls = if done {
        "min-w-0 truncate text-left text-sm text-muted-foreground line-through"
    } else {
        "min-w-0 truncate text-left text-sm text-foreground"
    };

    let tracked = t.tracked_seconds(chrono::Utc::now());
    let anchor = props.anchor.clone();
    let odd_status =
        status != Status::Open && status != Status::Done && status != Status::InProgress;
    // What kind of task / where it belongs — beside the title.
    let identity = rsx! {
        if priority != Priority::Normal && priority != Priority::None {
            span { class: "{CHIP} uppercase tracking-wide {priority_pill(priority)}", "{priority.label()}" }
        }
        if odd_status {
            span { class: "{CHIP} {status_pill(status)}", "{status.label()}" }
        }
        for c in t.contexts.iter() {
            span { key: "{c}", class: "{CHIP} bg-muted text-muted-foreground", "@{c.trim_start_matches('@')}" }
        }
        for p in t.projects.iter() {
            span { key: "{p}", class: "{CHIP} bg-primary/10 text-foreground/85",
                span { class: "text-primary", Hash { size: 11 } }
                "{strip_wikilink(p)}"
            }
        }
        if let Some(a) = anchor.clone() {
            span {
                class: "{CHIP} bg-muted text-muted-foreground",
                title: "{anchor_tooltip(&a.kind)}",
                match a.kind.as_str() {
                    "workstream" => rsx! { Waypoints { size: 11 } },
                    "milestone" => rsx! { Flag { size: 11 } },
                    _ => rsx! { CornerDownRight { size: 11 } },
                }
                "{a.label}"
            }
        }
    };
    // Who and when — at the right edge.
    let trailing = rsx! {
        if let Some(org) = props.org.clone() {
            span { class: "max-w-[8rem] truncate text-xs text-muted-foreground", title: "Org", "{org}" }
        }
        if tracked > 0 {
            span {
                class: if live {
                    "{CHIP} tabular-nums bg-sky-500/15 text-foreground"
                } else {
                    "{CHIP} tabular-nums bg-muted text-muted-foreground"
                },
                {crate::display::duration_label(tracked)}
            }
        }
        if let Some(d) = t.due_date() {
            {
                let (label, cls) = due_pill(d, done);
                rsx! { span { class: "{CHIP} {cls}", "{label}" } }
            }
        }
        if let Some(name) = owner.clone() {
            span {
                class: "flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold text-foreground/80",
                title: "Owner: {name}",
                aria_label: "Owner: {name}",
                "{initials(&name)}"
            }
        }
    };

    rsx! {
        div {
            class: "group flex min-h-[44px] items-center gap-2.5 rounded-md border border-transparent px-2 py-2 hover:border-border hover:bg-accent/30 cursor-pointer sm:min-h-0 sm:py-1.5",
            onclick: move |_| props.on_open.call(id),
            CheckboxButton {
                status,
                priority,
                live,
                title: t.title.clone(),
                on_click: move |()| props.on_toggle.call(id),
            }
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                div { class: "flex min-w-0 items-center gap-2",
                    // The title is the row's keyboard handle: Tab lands
                    // here, Enter opens the task.
                    button {
                        r#type: "button",
                        class: "{title_cls} rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        onclick: move |e: MouseEvent| {
                            e.stop_propagation();
                            props.on_open.call(id);
                        },
                        "{t.title}"
                    }
                    if let Some(n) = late {
                        span { class: "shrink-0 text-xs font-semibold tabular-nums text-rose-500",
                            {late_label(n)}
                        }
                    }
                    div { class: "hidden shrink-0 items-center gap-1.5 sm:flex", {identity.clone()} }
                    div { class: "ml-auto hidden shrink-0 items-center gap-2 sm:flex", {trailing.clone()} }
                }
                // Touch widths: everything wraps below the title.
                div { class: "flex flex-wrap items-center gap-1.5 empty:hidden sm:hidden",
                    {identity}
                    {trailing}
                }
            }
            if !done {
                TrackButton { live: status == Status::InProgress, on_click: move |()| props.on_track.call(id) }
            }
            RowMenu {
                id,
                done,
                working: status == Status::InProgress,
                on_track: props.on_track,
                on_event: props.on_event,
            }
        }
    }
}

/// "3d late", "5w late" — the age that turns an overdue date into a
/// priority.
#[must_use]
pub fn late_label(days: i64) -> String {
    match days {
        0 => "late".to_owned(),
        1..=13 => format!("{days}d late"),
        14..=89 => format!("{}w late", days / 7),
        _ => format!("{}mo late", days / 30),
    }
}

/// Start / stop work. Shown on hover and focus on a desk; on a phone
/// the same action is in the ⋯ menu (no hover to reveal it, and the
/// row is already full).
#[component]
fn TrackButton(live: bool, on_click: EventHandler<()>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if live {
                "hidden h-7 w-7 shrink-0 items-center justify-center rounded-md text-sky-500 transition-colors hover:bg-sky-500/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:flex"
            } else {
                "hidden h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:flex sm:opacity-0 sm:group-hover:opacity-100 sm:group-focus-within:opacity-100"
            },
            title: if live { "Stop working on this" } else { "Start working on this" },
            aria_label: if live { "Stop working on this" } else { "Start working on this" },
            onclick: move |e: MouseEvent| {
                e.stop_propagation();
                on_click.call(());
            },
            if live {
                Square { size: 13 }
            } else {
                Play { size: 14 }
            }
        }
    }
}

/// Per-row overflow menu (⋯) — start/stop, complete/reopen, won't do,
/// and a two-click delete. Hidden until hover on a desk (and shown on
/// keyboard focus), always visible on touch. Stops click propagation
/// so acting on a row never also opens its detail panel.
#[component]
fn RowMenu(
    id: Uuid,
    done: bool,
    working: bool,
    on_track: EventHandler<Uuid>,
    on_event: EventHandler<TaskMutation>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    rsx! {
        div { class: "relative shrink-0",
            button {
                r#type: "button",
                class: "-my-2 flex h-11 w-11 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:my-0 sm:h-7 sm:w-7 sm:opacity-0 sm:group-hover:opacity-100 sm:group-focus-within:opacity-100",
                title: "Task actions",
                aria_label: "Task actions",
                aria_haspopup: "menu",
                aria_expanded: open().to_string(),
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
                    class: "absolute right-0 top-full z-40 mt-1 flex w-48 flex-col overflow-hidden rounded-lg border border-border bg-popover py-1 text-sm shadow-xl",
                    role: "menu",
                    onclick: move |e: MouseEvent| e.stop_propagation(),
                    if !done {
                        button {
                            r#type: "button",
                            role: "menuitem",
                            class: "flex items-center gap-2 px-3 py-2 text-left hover:bg-accent sm:py-1.5",
                            onclick: move |_| {
                                on_track.call(id);
                                open.set(false);
                            },
                            if working { Square { size: 13 } } else { Play { size: 13 } }
                            if working { "Stop working" } else { "Start working" }
                        }
                    }
                    button {
                        r#type: "button",
                        role: "menuitem",
                        class: "flex items-center gap-2 px-3 py-2 text-left hover:bg-accent sm:py-1.5",
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
                    if !done {
                        button {
                            r#type: "button",
                            role: "menuitem",
                            class: "flex items-center gap-2 px-3 py-2 text-left hover:bg-accent sm:py-1.5",
                            onclick: move |_| {
                                on_event.call(TaskMutation::SetStatus { id, status: "cancelled".into() });
                                open.set(false);
                            },
                            Ban { size: 13 }
                            "Won't do"
                        }
                    }
                    div { class: "my-1 h-px bg-border/60" }
                    button {
                        r#type: "button",
                        role: "menuitem",
                        class: "flex items-center gap-2 px-3 py-2 text-left text-destructive hover:bg-destructive/10 sm:py-1.5",
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
    /// A clock is running on the task (see [`clock_running`]). Only
    /// then does the in-progress mark pulse.
    #[props(default)]
    live: bool,
    /// The task's title, for the accessible name ("Complete ‹title›").
    #[props(default)]
    title: String,
    on_click: EventHandler<()>,
}

/// The checkbox: empty (open), half-filled (in progress — pulsing only
/// while a clock actually runs), checked (done). A click completes; a
/// click on a done task reopens (`task_proto::click_transition`).
///
/// The visible box is 20–24px; the button around it is 44px on touch
/// so a thumb can't miss it.
#[component]
pub fn CheckboxButton(props: CheckboxButtonProps) -> Element {
    let done = matches!(props.status, Status::Done | Status::Cancelled);
    let base = "flex h-6 w-6 items-center justify-center rounded-md border-2 transition-colors sm:h-5 sm:w-5";
    let (cls, glyph) = match props.status {
        Status::Done | Status::Cancelled => (
            format!("{base} border-emerald-500 bg-emerald-500 text-white"),
            Some(rsx! { Check { size: 12 } }),
        ),
        Status::InProgress => (
            format!("{base} border-sky-500 group-hover/check:bg-sky-500/10"),
            Some(if props.live {
                rsx! { span { class: "h-2 w-2 rounded-full bg-sky-500 motion-safe:animate-pulse" } }
            } else {
                rsx! { span { class: "h-1/2 w-full self-end rounded-b-[3px] bg-sky-500/60" } }
            }),
        ),
        _ => {
            let edge = match props.priority {
                Priority::Critical => "border-rose-500",
                Priority::High => "border-amber-500",
                _ => "border-muted-foreground/60",
            };
            (
                format!("{base} bg-transparent group-hover/check:bg-accent {edge}"),
                None,
            )
        }
    };
    let label = if done {
        format!("Reopen {}", props.title)
    } else {
        format!("Complete {}", props.title)
    };
    rsx! {
        button {
            r#type: "button",
            role: "checkbox",
            aria_checked: done.to_string(),
            aria_label: "{label}",
            title: if props.status == Status::InProgress && props.live { "Working on it — click to complete" } else { "" },
            class: "group/check -m-2.5 flex shrink-0 items-center justify-center rounded-lg p-2.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:m-0 sm:p-0",
            onclick: move |e: MouseEvent| {
                e.stop_propagation();
                props.on_click.call(());
            },
            span { class: "{cls}", {glyph} }
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

/// The due-date pill — rose when overdue, amber today, sky this week,
/// quiet beyond and once done. The tint carries the urgency and the
/// text stays the theme's foreground, so it reads in every theme and
/// mode (Tailwind's `dark:` follows the OS, not the app's own mode).
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
        "text-muted-foreground ring-1 ring-inset ring-border"
    } else if d < today {
        "bg-rose-500/20 font-medium text-foreground"
    } else if d == today {
        "bg-amber-500/25 font-medium text-foreground"
    } else if d <= today + chrono::Duration::days(7) {
        "bg-sky-500/15 text-foreground"
    } else {
        "text-muted-foreground ring-1 ring-inset ring-border"
    };
    (label, cls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lateness_reads_in_the_unit_that_matters() {
        assert_eq!(late_label(1), "1d late");
        assert_eq!(late_label(13), "13d late");
        assert_eq!(late_label(39), "5w late");
        assert_eq!(late_label(120), "4mo late");
    }

    #[test]
    fn initials_take_first_and_last_words() {
        assert_eq!(initials("Alice Liddell"), "AL");
        assert_eq!(initials("Sam"), "SA");
        assert_eq!(initials("Mary Jo Kim"), "MK");
    }

    #[test]
    fn only_an_open_entry_is_a_running_clock() {
        let mut t = TaskInfo::new("t");
        t.status = "in-progress".into();
        assert!(!clock_running(&t), "in-progress alone is not a clock");
        t.time_entries.0.push(task_proto::TimeEntry {
            start_time: chrono::Utc::now(),
            end_time: None,
        });
        assert!(clock_running(&t));
    }
}
