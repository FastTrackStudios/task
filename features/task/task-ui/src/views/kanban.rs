//! Kanban board — one column per [`Status`] in
//! `crate::display::BOARD_ORDER`. Cards mirror the row content but
//! stack the meta below the title.
//!
//! Cards move between columns three ways: dragged (desk), Shift+←/→
//! on a focused card (the board's keyboard model, see
//! [`super::tasks_app`]), and the status buttons in the task sheet
//! (touch, where HTML drag and drop doesn't reach).

use dioxus::prelude::*;
use uuid::Uuid;

use crate::TaskInfo;
use task_proto::{Priority, Status};

use crate::display::{PriorityLabel, StatusLabel, TaskDisplay};

use super::palette::priority_pill;

#[derive(Props, Clone, PartialEq)]
pub struct KanbanBoardProps {
    pub tasks: Vec<TaskInfo>,
    pub on_open: EventHandler<Uuid>,
    pub on_set_status: EventHandler<(Uuid, String)>,
}

/// The data-transfer type a dragged card carries (its task id).
const DRAG_MIME: &str = "application/x-task-id";

/// The card in flight, and the column it is over.
#[derive(Clone, Copy, PartialEq)]
struct Drag {
    card: Signal<Option<(Uuid, Status)>>,
    over: Signal<Option<Status>>,
}

impl Drag {
    fn end(mut self) {
        self.card.set(None);
        self.over.set(None);
    }
}

#[component]
pub fn KanbanBoard(props: KanbanBoardProps) -> Element {
    let drag = Drag {
        card: use_signal(|| None),
        over: use_signal(|| None),
    };
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
                    drag,
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
    drag: Drag,
    on_open: EventHandler<Uuid>,
    on_set_status: EventHandler<(Uuid, String)>,
}

#[component]
fn Column(props: ColumnProps) -> Element {
    let status = props.status;
    let mut drag = props.drag;
    // A drop target only for a card from another column.
    let from_elsewhere = drag.card.read().is_some_and(|(_, from)| from != status);
    let target = from_elsewhere && *drag.over.read() == Some(status);
    let frame = if target {
        "border-primary/60 bg-primary/5"
    } else {
        "border-border bg-muted/20"
    };
    let header_dot = match status {
        Status::Open => "bg-slate-400",
        Status::InProgress => "bg-sky-400",
        Status::Waiting => "bg-amber-400",
        Status::Done => "bg-emerald-400",
        Status::Cancelled => "bg-rose-400",
    };
    rsx! {
        div {
            class: "flex w-[82vw] max-w-xs shrink-0 snap-start flex-col gap-2 rounded-lg border p-2 transition-colors sm:w-72 {frame}",
            ondragover: move |e: Event<DragData>| {
                if drag.card.peek().is_some_and(|(_, from)| from != status) {
                    e.prevent_default();
                    if *drag.over.peek() != Some(status) {
                        drag.over.set(Some(status));
                    }
                }
            },
            ondrop: move |e: Event<DragData>| {
                e.prevent_default();
                let carried = e.data().data_transfer().get_data(DRAG_MIME);
                let card = *drag.card.peek();
                drag.end();
                let Some((id, from)) = card else { return };
                // The drag the page started, not something dropped in
                // from outside it.
                if carried.as_deref() != Some(id.to_string().as_str()) || from == status {
                    return;
                }
                props.on_set_status.call((id, status.as_str().to_owned()));
            },
            div { class: "flex items-center gap-2 px-1 py-1",
                span { class: "h-2 w-2 rounded-full {header_dot}" }
                span { class: "text-xs font-semibold uppercase tracking-wider text-foreground",
                    "{status.label()}"
                }
                span { class: "ml-auto text-xs tabular-nums text-muted-foreground", "{props.items.len()}" }
            }
            div { class: "flex flex-col gap-1.5",
                for t in props.items.iter() {
                    Card {
                        key: "{t.id}",
                        task: t.clone(),
                        drag,
                        on_open: props.on_open,
                    }
                }
                if props.items.is_empty() || target {
                    div {
                        class: if target {
                            "rounded-md border border-dashed border-primary/60 px-3 py-6 text-center text-xs text-foreground"
                        } else {
                            "rounded-md border border-dashed border-border/60 px-3 py-6 text-center text-xs text-muted-foreground"
                        },
                        if target { "Move to {status.label()}" } else { "No tasks" }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct CardProps {
    task: TaskInfo,
    drag: Drag,
    on_open: EventHandler<Uuid>,
}

#[component]
fn Card(props: CardProps) -> Element {
    let t = props.task.clone();
    let id = t.id;
    let status = t.status_enum();
    let priority = t.priority_enum();
    let mut drag = props.drag;
    let lifted = drag.card.read().is_some_and(|(card, _)| card == id);
    rsx! {
        button {
            r#type: "button",
            class: if lifted {
                "flex cursor-grabbing flex-col gap-1.5 rounded-md border border-dashed border-border bg-card p-2.5 text-left opacity-40"
            } else {
                "flex cursor-grab flex-col gap-1.5 rounded-md border border-border bg-card p-2.5 text-left shadow-sm hover:border-primary/50 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            },
            draggable: true,
            // The board's keyboard model treats a card as a row.
            "data-task-row": "{id}",
            "data-row-focus": "true",
            "data-board-card": "true",
            aria_keyshortcuts: "Shift+ArrowLeft Shift+ArrowRight",
            ondragstart: move |e: Event<DragData>| {
                let dt = e.data().data_transfer();
                let _ = dt.set_data(DRAG_MIME, &id.to_string());
                dt.set_effect_allowed("move");
                drag.card.set(Some((id, status)));
            },
            ondragend: move |_| drag.end(),
            onclick: move |_| props.on_open.call(id),
            span { class: "text-sm text-foreground line-clamp-2", "{t.title}" }
            // How late, as in the list — the date alone doesn't say it.
            if let Some(n) = super::row::days_late(&t, chrono::Local::now().date_naive()) {
                span { class: "text-xs font-semibold tabular-nums text-rose-500", {super::row::late_label(n)} }
            }
            {
                let owner = super::row::owner_name(&t);
                let has_meta = t.due_date().is_some()
                    || !t.contexts.is_empty()
                    || !t.projects.is_empty()
                    || owner.is_some()
                    || (priority != Priority::Normal && priority != Priority::None);
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
                                        std::cmp::Ordering::Less => "rounded-full bg-rose-500/20 px-2 py-0.5 text-xs font-medium text-foreground",
                                        std::cmp::Ordering::Equal => "rounded-full bg-amber-500/25 px-2 py-0.5 text-xs font-medium text-foreground",
                                        std::cmp::Ordering::Greater => "rounded-full px-2 py-0.5 text-xs text-muted-foreground ring-1 ring-inset ring-border",
                                    };
                                    rsx! { span { class: "{cls}", "{label}" } }
                                }
                            }
                            if priority != Priority::Normal && priority != Priority::None {
                                span { class: "rounded-full px-2 py-0.5 text-xs uppercase tracking-wide {priority_pill(priority)}",
                                    "{priority.label()}"
                                }
                            }
                            for p in t.projects.iter() {
                                span {
                                    key: "{p}",
                                    class: "rounded-full bg-primary/10 px-2 py-0.5 text-xs text-foreground/85",
                                    "#{super::row::strip_wikilink(p)}"
                                }
                            }
                            for c in t.contexts.iter() {
                                span {
                                    key: "{c}",
                                    class: "rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground",
                                    "@{c.trim_start_matches('@')}"
                                }
                            }
                            if let Some(name) = owner {
                                span { class: "ml-auto text-xs text-muted-foreground", title: "Owner", "{name}" }
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
