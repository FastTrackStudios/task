//! The triage strip — unfiled tasks, and the one control that fixes
//! them.
//!
//! `task_proto::filing` calls a task unfiled when it hangs off
//! nothing: no project, no parent, no workstream, no milestone, not
//! even a GTD context. Those rows are excluded from the Relevant view
//! because `Telemetry + Observability: Sentry` on its own is not an
//! answer to "what should I do now" — but excluding without offering
//! anywhere to put them would just be losing work. So they surface
//! here, above the list, with a project picker per row: file it and it
//! rejoins the working list on the next render.
//!
//! Collapsed by default. The count is the point — a number you want to
//! drive to zero, not a section you read every morning.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{ChevronRight, Inbox};
use uuid::Uuid;

use crate::{TaskInfo, TaskMutation};

#[derive(Props, Clone, PartialEq)]
pub struct TriageStripProps {
    /// Open tasks with no filing anchor, oldest capture first (see
    /// `task_proto::relevance::partition_triage`).
    pub tasks: Vec<TaskInfo>,
    /// `(id, title)` project choices for the per-row picker. Empty
    /// hides the picker and leaves the strip read-only.
    #[props(default)]
    pub projects: Vec<(Uuid, String)>,
    pub on_event: EventHandler<TaskMutation>,
    pub on_open: EventHandler<Uuid>,
}

#[component]
pub fn TriageStrip(props: TriageStripProps) -> Element {
    let mut open = use_signal(|| false);
    let n = props.tasks.len();
    if n == 0 {
        return rsx! {};
    }
    let icon_rotation = if open() { "rotate-90" } else { "" };

    rsx! {
        div { class: "flex flex-col gap-1 rounded-lg border border-amber-500/30 bg-amber-500/5 px-2 py-1.5",
            button {
                r#type: "button",
                class: "flex items-center gap-1.5 text-xs font-medium text-amber-500/90 transition-colors hover:text-amber-400",
                onclick: move |_| open.toggle(),
                span { class: "transition-transform {icon_rotation}",
                    ChevronRight { size: 12 }
                }
                Inbox { size: 12 }
                if n == 1 {
                    "1 task needs filing"
                } else {
                    "{n} tasks need filing"
                }
                span { class: "font-normal text-muted-foreground",
                    "— hidden from Relevant until they say what they belong to"
                }
            }
            if open() {
                div { class: "flex flex-col gap-0.5 pl-1",
                    for t in props.tasks.iter().cloned() {
                        TriageRow {
                            key: "{t.id}",
                            task: t,
                            projects: props.projects.clone(),
                            on_event: props.on_event,
                            on_open: props.on_open,
                        }
                    }
                }
            }
        }
    }
}

/// One unfiled task: title (click to open the detail sheet) plus the
/// picker that files it. Choosing a project writes `project_id` and
/// mirrors the title into `projects:` so the markdown page reads
/// correctly in vanilla Obsidian — the same pair the quick-add sets.
#[component]
fn TriageRow(
    task: TaskInfo,
    projects: Vec<(Uuid, String)>,
    on_event: EventHandler<TaskMutation>,
    on_open: EventHandler<Uuid>,
) -> Element {
    let id = task.id;
    rsx! {
        div { class: "group flex min-h-[36px] items-center gap-2 rounded-md px-1.5 py-1 hover:bg-accent/30",
            span {
                class: "flex-1 min-w-0 truncate text-sm text-foreground cursor-pointer",
                onclick: move |_| on_open.call(id),
                "{task.title}"
            }
            if !projects.is_empty() {
                select {
                    class: "shrink-0 rounded-md border border-border/60 bg-transparent px-1.5 py-0.5 text-xs text-muted-foreground outline-none focus:border-border",
                    value: "",
                    onchange: move |e| {
                        let Ok(pid) = Uuid::parse_str(&e.value()) else { return };
                        let Some((_, title)) = projects.iter().find(|(p, _)| *p == pid) else {
                            return;
                        };
                        let mut filed = task.clone();
                        filed.project_id = Some(pid);
                        // Keep the human-readable hint in sync — the
                        // page is markdown first, a row second.
                        filed.projects = vec![format!("[[{title}]]")].into();
                        on_event.call(TaskMutation::Update { task: filed });
                    },
                    option { value: "", selected: true, "file under…" }
                    for (pid, title) in projects.iter() {
                        option { key: "{pid}", value: "{pid}", "{title}" }
                    }
                }
            }
        }
    }
}
