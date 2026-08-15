//! Kanban board — one column per [`Status`] in
//! `crate::display::BOARD_ORDER`. Cards mirror the row content but
//! stack the meta below the title.

use dioxus::prelude::*;
use uuid::Uuid;

use crate::TaskInfo;
use task_proto::{Priority, Status};

use crate::display::{PriorityLabel, StatusLabel, TaskDisplay};

use super::palette::{priority_pill, priority_rail};

#[derive(Props, Clone, PartialEq)]
pub struct KanbanBoardProps {
    pub tasks: Vec<TaskInfo>,
    pub on_open: EventHandler<Uuid>,
    pub on_set_status: EventHandler<(Uuid, String)>,
}

#[component]
pub fn KanbanBoard(props: KanbanBoardProps) -> Element {
    let columns: Vec<(Status, Vec<TaskInfo>)> = crate::display::BOARD_ORDER
        .iter()
        .map(|s| {
            let bucket = props
                .tasks
                .iter()
                .filter(|t| t.status_enum() == *s)
                .cloned()
                .collect::<Vec<_>>();
            (*s, bucket)
        })
        .collect();

    rsx! {
        div { class: "flex snap-x snap-mandatory gap-3 overflow-x-auto pb-2 [scrollbar-width:thin]",
            for (status, items) in columns.into_iter() {
                Column {
                    key: "{status.as_str()}",
                    status,
                    items,
                    on_open: props.on_open,
                    on_set_status: props.on_set_status,
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ColumnProps {
    status: Status,
    items: Vec<TaskInfo>,
    on_open: EventHandler<Uuid>,
    on_set_status: EventHandler<(Uuid, String)>,
}

#[component]
fn Column(props: ColumnProps) -> Element {
    let status = props.status;
    let header_dot = match status {
        Status::Open => "bg-slate-400",
        Status::InProgress => "bg-sky-400",
        Status::Waiting => "bg-amber-400",
        Status::Done => "bg-emerald-400",
        Status::Cancelled => "bg-rose-400",
    };
    rsx! {
        div { class: "flex flex-col gap-2 w-[82vw] max-w-xs sm:w-72 shrink-0 snap-start rounded-lg border border-border bg-muted/20 p-2",
            div { class: "flex items-center gap-2 px-1 py-1",
                span { class: "h-2 w-2 rounded-full {header_dot}" }
                span { class: "text-xs font-semibold uppercase tracking-wider text-foreground",
                    "{status.label()}"
                }
                span { class: "ml-auto text-[11px] text-muted-foreground", "{props.items.len()}" }
            }
            div { class: "flex flex-col gap-1.5",
                for t in props.items.iter() {
                    Card {
                        key: "{t.id}",
                        task: t.clone(),
                        on_open: props.on_open,
                    }
                }
                if props.items.is_empty() {
                    div { class: "rounded-md border border-dashed border-border/60 px-3 py-6 text-center text-xs text-muted-foreground",
                        "No tasks"
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct CardProps {
    task: TaskInfo,
    on_open: EventHandler<Uuid>,
}

#[component]
fn Card(props: CardProps) -> Element {
    let t = props.task.clone();
    let id = t.id;
    let priority = t.priority_enum();
    let rail = priority_rail(priority);
    rsx! {
        button {
            r#type: "button",
            class: "relative flex flex-col gap-1.5 rounded-md border border-border bg-card p-2.5 pl-3 text-left shadow-sm hover:border-primary/50 hover:shadow-md",
            onclick: move |_| props.on_open.call(id),
            // Priority rail.
            span { class: "absolute left-0 top-1 bottom-1 w-0.5 rounded-r {rail}" }
            span { class: "text-sm text-foreground line-clamp-2", "{t.title}" }
            {
                let has_meta = t.due_date().is_some()
                    || !t.contexts.is_empty()
                    || !t.projects.is_empty()
                    || priority != Priority::Normal;
                if has_meta {
                    rsx! {
                        div { class: "flex flex-wrap items-center gap-1",
                            if let Some(d) = t.due_date() {
                                {
                                    let today = chrono::Local::now().date_naive();
                                    let label = if d == today {
                                        "Today".to_string()
                                    } else { d.format("%b %-d").to_string() };
                                    let cls = match d.cmp(&today) {
                                        std::cmp::Ordering::Less => "rounded-full border border-rose-500 bg-rose-700/50 px-1.5 py-0 text-[10px] text-rose-100",
                                        std::cmp::Ordering::Equal => "rounded-full border border-amber-400 bg-amber-700/50 px-1.5 py-0 text-[10px] text-amber-50",
                                        std::cmp::Ordering::Greater => "rounded-full border border-border bg-muted/40 px-1.5 py-0 text-[10px] text-muted-foreground",
                                    };
                                    rsx! { span { class: "{cls}", "{label}" } }
                                }
                            }
                            if priority != Priority::Normal {
                                span { class: "rounded-full px-1.5 py-0 text-[10px] uppercase tracking-wider {priority_pill(priority)}",
                                    "{priority.label()}"
                                }
                            }
                            for c in t.contexts.iter() {
                                span {
                                    key: "{c}",
                                    class: "rounded-full bg-muted/50 px-1.5 py-0 text-[10px] text-muted-foreground",
                                    "@{c}"
                                }
                            }
                        }
                    }
                } else {
                    rsx! {}
                }
            }
        }
    }
}
