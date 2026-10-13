//! The issue view — one task, on Linear's layout, built from the shared
//! property system.
//!
//! - **Main column:** the title (24/32, editable in place), the
//!   description (15/24), sub-issues with their progress and an inline
//!   "Add sub-issue", then the activity feed the page supplies.
//! - **Properties:** a column on a desk — Status, Priority, Assignee,
//!   Due; Labels; Project — and on a phone the same chips in a row
//!   under the title, as Linear's iOS app does.
//! - **Keys:** S, P, A, R (project) and L open their pickers whenever no
//!   text field has focus — the [`Hotkey`] signal the chips follow.
//!
//! Dumb by design: data in through props, every edit out as a
//! [`TaskMutation`], so the page decides how it persists (the shared
//! optimistic store) and the same view can sit in a page or a sheet.

use architect_ui::lucide_dioxus::{CalendarDays, Plus};
use dioxus::prelude::*;
use uuid::Uuid;

use super::properties::{
    AssigneeChip, Hotkey, LabelsChip, PersonDot, PriorityChip, ProjectChip, PropertyRow,
    StatusChip, StatusIcon,
};
use crate::{TaskInfo, TaskMutation};

/// Tags that mark a record's kind rather than label it — never shown as
/// labels, never removed by the labels picker.
const MARKER_TAGS: &[&str] = &["task", "subtask"];

/// The labels a task shows: its tags, minus the kind markers.
#[must_use]
pub fn labels_of(task: &TaskInfo) -> Vec<String> {
    task.tags
        .iter()
        .filter(|t| !MARKER_TAGS.contains(&t.as_str()))
        .cloned()
        .collect()
}

/// `task` with `label` added if absent, removed if present (case-blind).
#[must_use]
pub fn toggle_label(task: &TaskInfo, label: &str) -> TaskInfo {
    let mut next = task.clone();
    let label = label.trim().trim_start_matches('#');
    if label.is_empty() {
        return next;
    }
    if let Some(i) = next.tags.iter().position(|t| t.eq_ignore_ascii_case(label)) {
        next.tags.0.remove(i);
    } else {
        next.tags.0.push(label.to_owned());
    }
    next
}

/// The assignee's user id: the first human assignee.
#[must_use]
pub fn assignee_of(task: &TaskInfo) -> Option<String> {
    task.workflow
        .as_ref()?
        .assignees
        .iter()
        .find_map(|a| match a {
            task_proto::workflows_proto::AgentRef::Human { user_id, .. } => Some(user_id.clone()),
            task_proto::workflows_proto::AgentRef::Agent { .. } => None,
        })
}

/// `task` assigned to `who` (`(user id, name)`), or unassigned. Agent
/// claims are kept — a person and an agent can share an issue.
#[must_use]
pub fn with_assignee(task: &TaskInfo, who: Option<(String, String)>) -> TaskInfo {
    use task_proto::workflows_proto::AgentRef;
    let mut next = task.clone();
    let wf = next.workflow.get_or_insert_with(Default::default);
    wf.assignees
        .0
        .retain(|a| matches!(a, AgentRef::Agent { .. }));
    if let Some((user_id, name)) = who {
        wf.assignees.0.insert(
            0,
            AgentRef::Human {
                user_id,
                display_name: Some(name),
            },
        );
    }
    next
}

#[derive(Props, Clone, PartialEq)]
pub struct IssueViewProps {
    pub task: TaskInfo,
    /// `(user id, name)` of everyone it could be assigned to; the
    /// signed-in person first.
    #[props(default)]
    pub people: Vec<(String, String)>,
    /// `(id, title)` of the projects it could belong to.
    #[props(default)]
    pub projects: Vec<(Uuid, String)>,
    /// Every label in use, for the labels picker.
    #[props(default)]
    pub known_labels: Vec<String>,
    /// Its sub-issues, in order.
    #[props(default)]
    pub subtasks: Vec<TaskInfo>,
    /// The activity feed, rendered by the page (it owns the fetch).
    #[props(default)]
    pub activity: Option<Element>,
    /// The description editor, rendered by the page — the shared
    /// document editor (`crates/ui`'s `DocBody`), which this crate does
    /// not depend on. The page commits its edits itself; when set it
    /// replaces the plain description field, and the title commit leaves
    /// `details` to it.
    #[props(default)]
    pub body: Option<Element>,
    pub on_event: EventHandler<TaskMutation>,
    /// A sub-issue was clicked.
    pub on_open: EventHandler<Uuid>,
    /// "Add sub-issue" with this title.
    pub on_add_subtask: EventHandler<String>,
}

#[component]
pub fn IssueView(props: IssueViewProps) -> Element {
    let task = props.task.clone();
    let id = task.id;
    let mut hotkey = use_signal(|| None::<Hotkey>);
    // Whether a text field has focus — the property keys stand down
    // while you type.
    let mut editing = use_signal(|| false);
    let mut title = use_signal(|| task.title.clone());
    let mut details = use_signal(|| task.details.clone());
    let mut adding = use_signal(|| false);
    let mut sub_title = use_signal(String::new);
    // A different task through the same view starts from its own text.
    let mut shown_id = use_signal(|| id);
    if *shown_id.peek() != id {
        shown_id.set(id);
        title.set(task.title.clone());
        details.set(task.details.clone());
    }

    let on_event = props.on_event;
    // With a page-supplied body the description is the page's to commit;
    // the `details` signal here never sees its edits, so the title commit
    // must carry the task's current details rather than a stale copy.
    let has_body = props.body.is_some();
    let commit_text = {
        let task = task.clone();
        move || {
            let t = title.peek().trim().to_owned();
            let d = if has_body {
                task.details.clone()
            } else {
                details.peek().clone()
            };
            if (!t.is_empty() && t != task.title) || d != task.details {
                let mut next = task.clone();
                if !t.is_empty() {
                    next.title = t;
                }
                next.details = d;
                on_event.call(TaskMutation::Update { task: next });
            }
        }
    };
    let commit_title = commit_text.clone();
    let commit_details = commit_text.clone();
    let commit_enter = commit_text.clone();

    let people = props.people.clone();
    let done = props
        .subtasks
        .iter()
        .filter(|s| task_proto::status_is_terminal(&s.status))
        .count();

    // The chips, defined once and placed twice (phone row, desk column).
    let chips = {
        let task_s = task.clone();
        let task_p = task.clone();
        let task_a = task.clone();
        let task_r = task.clone();
        let task_l = task.clone();
        let people_a = people.clone();
        let projects = props.projects.clone();
        let projects_r = projects.clone();
        move |column: bool| {
            let task_s = task_s.clone();
            let task_p = task_p.clone();
            let task_a = task_a.clone();
            let task_r = task_r.clone();
            let task_l = task_l.clone();
            let people_a = people_a.clone();
            let projects_r = projects_r.clone();
            let row = |label: &'static str, chip: Element| {
                if column {
                    rsx! {
                        PropertyRow { label, {chip} }
                    }
                } else {
                    chip
                }
            };
            rsx! {
                {row("Status", rsx! { StatusChip {
                    status: task_s.status.clone(),
                    hotkey,
                    on_change: move |s: String| on_event.call(TaskMutation::SetStatus { id, status: s }),
                } })}
                {row("Priority", rsx! { PriorityChip {
                    priority: task_p.priority.clone(),
                    hotkey,
                    on_change: move |p: String| on_event.call(TaskMutation::SetPriority { id, priority: p }),
                } })}
                {row("Assignee", rsx! { AssigneeChip {
                    assignee: assignee_of(&task_a),
                    people: people_a.clone(),
                    hotkey,
                    on_change: {
                        let task_a = task_a.clone();
                        let people_a = people_a.clone();
                        move |who: Option<String>| {
                            let pair = who.and_then(|id| people_a.iter().find(|(p, _)| *p == id).cloned());
                            on_event.call(TaskMutation::Update { task: with_assignee(&task_a, pair) });
                        }
                    },
                } })}
                {row("Project", rsx! { ProjectChip {
                    project: task_r.project_id,
                    projects: projects_r.clone(),
                    hotkey,
                    on_change: {
                        let task_r = task_r.clone();
                        let projects_r = projects_r.clone();
                        move |p: Option<Uuid>| {
                            let mut next = task_r.clone();
                            next.project_id = p;
                            next.projects = task_proto::model::StringList(
                                p.and_then(|p| projects_r.iter().find(|(id, _)| *id == p))
                                    .map(|(_, n)| vec![format!("[[{n}]]")])
                                    .unwrap_or_default(),
                            );
                            on_event.call(TaskMutation::Update { task: next });
                        }
                    },
                } })}
                {row("Labels", rsx! { LabelsChip {
                    chosen: labels_of(&task_l),
                    known: props.known_labels.clone(),
                    hotkey,
                    on_toggle: {
                        let task_l = task_l.clone();
                        move |l: String| on_event.call(TaskMutation::Update { task: toggle_label(&task_l, &l) })
                    },
                } })}
                {row("Due", rsx! { DueChip {
                    due: task_l.due.clone(),
                    on_change: {
                        let task_l = task_l.clone();
                        move |d: Option<String>| {
                            let mut next = task_l.clone();
                            next.due = d;
                            on_event.call(TaskMutation::Update { task: next });
                        }
                    },
                } })}
            }
        }
    };

    let on_key = move |e: KeyboardEvent| {
        if *editing.peek() || e.modifiers().ctrl() || e.modifiers().meta() || e.modifiers().alt() {
            return;
        }
        if let Key::Character(c) = e.key() {
            if let Some(h) = Hotkey::from_key(&c) {
                e.prevent_default();
                hotkey.set(Some(h));
            }
        }
    };

    rsx! {
        div {
            class: "mx-auto grid w-full max-w-6xl grid-cols-1 gap-8 px-4 py-5 outline-none sm:px-8 md:grid-cols-[minmax(0,1fr)_16rem] md:gap-12 md:py-10",
            tabindex: "0",
            onkeydown: on_key,
            // ── main column ──
            div { class: "flex min-w-0 flex-col",
                textarea {
                    aria_label: "Title",
                    rows: "1",
                    class: "w-full resize-none bg-transparent text-[24px] font-semibold leading-8 tracking-[-0.16px] text-foreground outline-none [field-sizing:content] placeholder:text-muted-foreground",
                    placeholder: "Issue title",
                    value: "{title}",
                    oninput: move |e| title.set(e.value()),
                    onfocusin: move |_| editing.set(true),
                    onfocusout: move |_| {
                        editing.set(false);
                        commit_title();
                    },
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter {
                            e.prevent_default();
                            commit_enter();
                        }
                    },
                }
                // Phone: the properties as a row of chips under the title.
                div { class: "mt-3 flex flex-wrap items-center gap-1.5 md:hidden", {chips(false)} }
                if let Some(body) = props.body.clone() {
                    // Keys typed into the editor are text, not property
                    // hotkeys: the same focus guard as the fields.
                    div {
                        class: "mt-4 min-w-0",
                        onfocusin: move |_| editing.set(true),
                        onfocusout: move |_| editing.set(false),
                        {body}
                    }
                } else {
                    textarea {
                        aria_label: "Description",
                        class: "mt-4 min-h-24 w-full resize-none bg-transparent text-[15px] font-[450] leading-6 tracking-[-0.1px] text-foreground/90 outline-none [field-sizing:content] placeholder:text-muted-foreground",
                        placeholder: "Add description…",
                        value: "{details}",
                        oninput: move |e| details.set(e.value()),
                        onfocusin: move |_| editing.set(true),
                        onfocusout: move |_| {
                            editing.set(false);
                            commit_details();
                        },
                    }
                }

                // ── sub-issues ──
                section { class: "mt-8 flex flex-col",
                    div { class: "flex items-center gap-2 border-b border-border/60 pb-2",
                        span { class: "text-[13px] font-medium text-muted-foreground", "Subtasks" }
                        if !props.subtasks.is_empty() {
                            span { class: "font-mono text-[11px] tabular-nums text-muted-foreground", "{done}/{props.subtasks.len()}" }
                        }
                        button {
                            r#type: "button",
                            class: "ml-auto flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground md:size-7",
                            title: "Add subtask",
                            aria_label: "Add subtask",
                            onclick: move |_| adding.set(true),
                            Plus { size: 15 }
                        }
                    }
                    for s in props.subtasks.iter().cloned() {
                        {
                            let sid = s.id;
                            let who = s.workflow.as_ref().and_then(|w| w.assignees.iter().next().map(|a| match a {
                                task_proto::workflows_proto::AgentRef::Human { display_name, user_id } =>
                                    display_name.clone().unwrap_or_else(|| user_id.clone()),
                                task_proto::workflows_proto::AgentRef::Agent { name, .. } => name.clone(),
                            }));
                            rsx! {
                                button {
                                    key: "{sid}",
                                    r#type: "button",
                                    class: "flex min-h-11 w-full items-center gap-2.5 rounded-md px-1.5 text-left text-[14px] text-foreground hover:bg-accent/50 md:min-h-9",
                                    onclick: move |_| props.on_open.call(sid),
                                    StatusIcon { status: s.status.clone() }
                                    span { class: "min-w-0 flex-1 truncate", "{s.title}" }
                                    if let Some(n) = who {
                                        PersonDot { name: n }
                                    }
                                }
                            }
                        }
                    }
                    if adding() {
                        div { class: "flex items-center gap-2.5 px-1.5 py-1",
                            StatusIcon { status: "open".to_owned() }
                            input {
                                r#type: "text",
                                class: "min-h-9 min-w-0 flex-1 bg-transparent text-[14px] text-foreground outline-none placeholder:text-muted-foreground",
                                placeholder: "Sub-issue title",
                                value: "{sub_title}",
                                autofocus: true,
                                onmounted: move |e| async move { let _ = e.set_focus(true).await; },
                                oninput: move |e| sub_title.set(e.value()),
                                onfocusin: move |_| editing.set(true),
                                onfocusout: move |_| editing.set(false),
                                onkeydown: move |e: KeyboardEvent| match e.key() {
                                    Key::Enter => {
                                        e.prevent_default();
                                        let t = sub_title.peek().trim().to_owned();
                                        if !t.is_empty() {
                                            props.on_add_subtask.call(t);
                                            sub_title.set(String::new());
                                        }
                                    }
                                    Key::Escape => {
                                        e.prevent_default();
                                        adding.set(false);
                                        sub_title.set(String::new());
                                    }
                                    _ => {}
                                },
                            }
                        }
                    } else if props.subtasks.is_empty() {
                        button {
                            r#type: "button",
                            class: "mt-1 self-start rounded-md px-1.5 py-2 text-[13px] text-muted-foreground hover:text-foreground",
                            onclick: move |_| adding.set(true),
                            "Break it into subtasks…"
                        }
                    }
                }

                // ── activity ──
                if let Some(feed) = props.activity.clone() {
                    section { class: "mt-10 flex flex-col gap-3",
                        span { class: "text-[13px] font-medium text-muted-foreground", "Activity" }
                        {feed}
                    }
                }
            }

            // ── properties (desk) ──
            aside { class: "hidden flex-col gap-1.5 md:flex",
                span { class: "pb-1 text-[13px] font-medium text-muted-foreground", "Properties" }
                {chips(true)}
            }
        }
    }
}

/// Due date: a chip showing the date (or "Due date"), opening the
/// native date input — the platform's own picker on a phone.
#[component]
fn DueChip(due: Option<String>, on_change: EventHandler<Option<String>>) -> Element {
    let label = due
        .as_deref()
        .and_then(|d| d.get(..10))
        .and_then(|d| d.parse::<chrono::NaiveDate>().ok())
        .map(|d| d.format("%b %-d").to_string());
    rsx! {
        label { class: "relative inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-md border border-border/70 px-2 text-[13px] font-medium text-muted-foreground hover:bg-accent hover:text-foreground md:h-7",
            CalendarDays { size: 14 }
            span { class: if label.is_some() { "text-foreground" } else { "" },
                {label.clone().unwrap_or_else(|| "Due date".to_owned())}
            }
            input {
                r#type: "date",
                class: "absolute inset-0 cursor-pointer opacity-0",
                aria_label: "Due date",
                value: "{due.clone().unwrap_or_default()}",
                onchange: move |e| on_change.call(Some(e.value()).filter(|v| !v.is_empty())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_hide_the_kind_markers_and_toggle_case_blind() {
        let mut t = task_proto::capture("Mix #mixing");
        t.tags.0.push("subtask".into());
        assert_eq!(labels_of(&t), vec!["mixing".to_owned()]);
        let t = toggle_label(&t, "Mixing");
        assert!(labels_of(&t).is_empty(), "removed regardless of case");
        let t = toggle_label(&t, "#client");
        assert_eq!(labels_of(&t), vec!["client".to_owned()]);
    }

    #[test]
    fn assigning_replaces_the_person_and_keeps_agents() {
        use task_proto::workflows_proto::AgentRef;
        let mut t = task_proto::capture("Bounce stems");
        let wf = t.workflow.get_or_insert_with(Default::default);
        wf.assignees.0.push(AgentRef::Agent {
            name: "claude".into(),
            model_version: None,
        });
        let t = with_assignee(&t, Some(("u1".into(), "Cody".into())));
        assert_eq!(assignee_of(&t).as_deref(), Some("u1"));
        let t = with_assignee(&t, Some(("u2".into(), "Sam".into())));
        assert_eq!(assignee_of(&t).as_deref(), Some("u2"));
        let wf = t.workflow.as_ref().unwrap();
        assert_eq!(wf.assignees.len(), 2, "one person, the agent kept");
        let t = with_assignee(&t, None);
        assert_eq!(assignee_of(&t), None);
    }
}
