//! Top-level Tasks app. Owns the view-mode toggle + the
//! currently-open detail panel; everything else is dumb.

use dioxus::prelude::*;
use architect_ui::prelude::*;
use uuid::Uuid;

use crate::TaskInfo;
use crate::TaskMutation;
use crate::display::TaskDisplay;

use super::detail::TaskDetail;
use super::kanban::KanbanBoard;
use super::list::TaskList;
use super::quick_add::QuickAdd;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    List,
    Kanban,
}

impl ViewMode {
    fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Kanban => "Kanban",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TasksAppProps {
    pub tasks: Vec<TaskInfo>,
    /// Per-anchor condensation leftovers (see
    /// [`super::list::TaskListProps::more`]): the list view renders
    /// them behind an inline "N more in {group}" expander.
    #[props(default)]
    pub more: Vec<(Uuid, Vec<TaskInfo>)>,
    /// Non-project belonging per task (see
    /// [`super::row::AnchorChip`]) — what keeps a row from reading as
    /// a bare title when it has no project.
    #[props(default)]
    pub anchors: Vec<(Uuid, super::row::AnchorChip)>,
    /// Open tasks that belong to nothing at all. Rendered above the
    /// list as the triage strip rather than mixed into it; see
    /// [`super::triage::TriageStrip`].
    #[props(default)]
    pub triage: Vec<TaskInfo>,
    pub on_event: EventHandler<TaskMutation>,
    #[props(default)]
    pub initial_view: Option<ViewMode>,
    /// When set, the quick-edit sheet offers "Open full view" and
    /// emits the task id — the page layer routes it (`/tasks/:id`).
    #[props(default)]
    pub on_open_full: Option<EventHandler<Uuid>>,
    /// Page-owned controls (filter chips) rendered inline in the
    /// header row — one row of chrome, not two.
    #[props(default)]
    pub header_extra: Option<Element>,
    /// `(id, title)` project choices forwarded to the quick-add's
    /// picker (empty = no picker).
    #[props(default)]
    pub projects: Vec<(uuid::Uuid, String)>,
}

#[component]
pub fn TasksApp(props: TasksAppProps) -> Element {
    let mut view = use_signal(|| props.initial_view.unwrap_or_default());
    let mut open_id: Signal<Option<Uuid>> = use_signal(|| None);

    // Condensed-away rows are still openable (from the expander), so
    // every lookup scans the visible list AND the leftovers.
    let find_task = {
        let tasks = props.tasks.clone();
        let more = props.more.clone();
        let triage = props.triage.clone();
        move |id: Uuid| -> Option<TaskInfo> {
            tasks
                .iter()
                .chain(more.iter().flat_map(|(_, rest)| rest.iter()))
                .chain(triage.iter())
                .find(|t| t.id == id)
                .cloned()
        }
    };
    let selected: Option<TaskInfo> = open_id.read().and_then(&find_task);

    let total = props.tasks.len();
    // Progress counts route through state groups (is_done), not
    // the Status enum, so custom completed-group names count.
    let done = props.tasks.iter().filter(|t| t.is_done()).count();
    let open_count = total - done;
    // Time tracked today across the board — the day's receipt.
    let now = chrono::Utc::now();
    let today = chrono::Local::now().date_naive();
    let tracked_today: i64 = props
        .tasks
        .iter()
        .flat_map(|t| t.time_entries.iter())
        .filter(|e| chrono::DateTime::<chrono::Local>::from(e.start_time).date_naive() == today)
        .map(|e| {
            (e.end_time.unwrap_or(now) - e.start_time)
                .num_seconds()
                .max(0)
        })
        .sum();

    rsx! {
        div { class: "relative mx-auto flex max-w-6xl flex-col gap-3 p-3 sm:p-5 lg:px-8 lg:py-6 h-full",
            div { class: "flex flex-wrap items-center gap-x-3 gap-y-2",
                Heading { level: HeadingLevel::H1, "Tasks" }
                span { class: "text-xs tabular-nums text-muted-foreground",
                    "{open_count} open · {done} done"
                    if tracked_today > 0 {
                        " · {crate::display::duration_label(tracked_today)} tracked today"
                    }
                }
                if let Some(extra) = props.header_extra.clone() {
                    {extra}
                }
                div { class: "ml-auto inline-flex items-center gap-0.5 rounded-lg bg-muted/40 p-0.5 text-xs",
                    for v in [ViewMode::List, ViewMode::Kanban] {
                        {
                            let active = v == view();
                            // Touch-sized below `sm`, compact on desktop.
                            let cls = if active {
                                "bg-background text-foreground shadow-sm font-medium px-3 py-1.5 sm:px-2.5 sm:py-0.5 rounded-md transition-colors"
                            } else {
                                "text-muted-foreground hover:text-foreground px-3 py-1.5 sm:px-2.5 sm:py-0.5 rounded-md transition-colors"
                            };
                            rsx! {
                                button {
                                    key: "{v.label()}",
                                    r#type: "button",
                                    class: "{cls}",
                                    onclick: move |_| view.set(v),
                                    "{v.label()}"
                                }
                            }
                        }
                    }
                }
            }
            // Hidden on phones: the page layer renders a sticky
            // bottom action bar (thumb reach) that opens the same
            // QuickAdd in a bottom sheet instead.
            div { class: "hidden sm:block",
                QuickAdd {
                    projects: props.projects.clone(),
                    on_create: move |task: TaskInfo| props.on_event.call(TaskMutation::Create { task }),
                }
            }
            // Unfiled work, above the list in both views — excluded
            // from Relevant, but never silently.
            super::triage::TriageStrip {
                tasks: props.triage.clone(),
                projects: props.projects.clone(),
                on_event: props.on_event,
                on_open: move |id: Uuid| open_id.set(Some(id)),
            }
            div { class: "flex-1 min-h-0 overflow-y-auto",
                match view() {
                    ViewMode::List => rsx! {
                        TaskList {
                            tasks: props.tasks.clone(),
                            more: props.more.clone(),
                            anchors: props.anchors.clone(),
                            on_event: props.on_event,
                            on_toggle: {
                                let find_task = find_task.clone();
                                move |id: Uuid| {
                                    // Domain click cycle: open → in-progress
                                    // (timer starts) → done; a subtask under a
                                    // running parent completes directly.
                                    let next = find_task(id).map(|t| {
                                        let parent_status = t.parent().and_then(|p| find_task(p));
                                        task_proto::click_transition(
                                            &t.status,
                                            parent_status.as_ref().map(|p| p.status.as_str()),
                                        )
                                        .to_string()
                                    });
                                    if let Some(s) = next {
                                        props.on_event.call(TaskMutation::SetStatus { id, status: s });
                                    }
                                }
                            },
                            on_open: move |id: Uuid| open_id.set(Some(id)),
                        }
                    },
                    ViewMode::Kanban => rsx! {
                        KanbanBoard {
                            tasks: props.tasks.clone(),
                            on_open: move |id: Uuid| open_id.set(Some(id)),
                            on_set_status: move |(id, status): (Uuid, String)| {
                                props.on_event.call(TaskMutation::SetStatus { id, status });
                            },
                        }
                    },
                }
            }
            if let Some(t) = selected {
                TaskDetail {
                    task: t,
                    on_event: props.on_event,
                    on_close: move |()| open_id.set(None),
                    on_open_full: props.on_open_full,
                }
            }
        }
    }
}
