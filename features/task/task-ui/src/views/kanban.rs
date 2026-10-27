//! Kanban board — one column per [`Status`] in
//! `crate::display::BOARD_ORDER`. Cards mirror the row content but
//! stack the meta below the title.
//!
//! Cards move between columns three ways: dragged with a mouse or pen,
//! Shift+←/→ on a focused card (the board's keyboard model, see
//! [`super::tasks_app`]), and the status buttons in the task sheet
//! (touch, where a drag would fight the board's own scroll).
//!
//! The drag is pointer events, not HTML drag and drop: Blitz has no
//! drag-and-drop events, and pointer events behave the same in a
//! browser. A press on a card arms it; moving past a few pixels lifts
//! it; the column under the pointer lights up; releasing there moves
//! the card. A press that never travels is a click, which opens it.

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

/// How far (CSS px) a press travels before it becomes a drag.
const LIFT_DISTANCE: f64 = 5.0;

/// A card pressed with a mouse or pen, and where the press began.
#[derive(Clone, Copy, PartialEq)]
struct Press {
    card: Uuid,
    from: Status,
    x: f64,
    y: f64,
}

/// The drag state the board's columns and cards share.
#[derive(Clone, Copy, PartialEq)]
struct Drag {
    /// The press in hand, lifted or not.
    press: Signal<Option<Press>>,
    /// The press has travelled: a card is in flight.
    lifted: Signal<bool>,
    /// The column under the pointer while a card is in flight.
    over: Signal<Option<Status>>,
    /// A drag just ended on its own card: swallow the click it makes.
    swallow_click: Signal<bool>,
}

impl Drag {
    /// The card in flight, if the press has lifted.
    fn card(&self) -> Option<Press> {
        if *self.lifted.read() {
            *self.press.read()
        } else {
            None
        }
    }

    /// The pointer moved over `column`: lift a press that travelled far
    /// enough, and track the column a lifted card is over.
    fn moved(mut self, column: Status, e: &PointerEvent) {
        let Some(press) = *self.press.peek() else {
            return;
        };
        if !*self.lifted.peek() {
            let at = e.data().client_coordinates();
            let travelled = (at.x - press.x).hypot(at.y - press.y);
            if travelled < LIFT_DISTANCE {
                return;
            }
            self.lifted.set(true);
        }
        if *self.over.peek() != Some(column) {
            self.over.set(Some(column));
        }
    }

    /// The pointer came up over `column`: the lifted card, if any, and
    /// where it was released.
    fn release(mut self) -> Option<Press> {
        let card = self.card();
        if card.is_some() {
            self.swallow_click.set(true);
        }
        self.cancel();
        card
    }

    fn cancel(mut self) {
        self.press.set(None);
        self.lifted.set(false);
        self.over.set(None);
    }
}

#[component]
pub fn KanbanBoard(props: KanbanBoardProps) -> Element {
    let drag = Drag {
        press: use_signal(|| None),
        lifted: use_signal(|| false),
        over: use_signal(|| None),
        swallow_click: use_signal(|| false),
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
        div {
            class: if drag.card().is_some() {
                "flex min-w-0 cursor-grabbing select-none gap-3 overflow-x-auto pb-2 [scrollbar-width:thin]"
            } else {
                "flex min-w-0 snap-x snap-mandatory gap-3 overflow-x-auto pb-2 [scrollbar-width:thin]"
            },
            // Released between columns, or the pointer left the board:
            // the card goes back where it was.
            onpointerup: move |_| drag.cancel(),
            onpointerleave: move |_| drag.cancel(),
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
    let drag = props.drag;
    // A drop target only for a card from another column.
    let from_elsewhere = drag.card().is_some_and(|p| p.from != status);
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
            onpointermove: move |e: PointerEvent| drag.moved(status, &e),
            onpointerup: move |e: PointerEvent| {
                // Handled here; the board's own handler would cancel.
                e.stop_propagation();
                if let Some(press) = drag.release() {
                    if press.from != status {
                        props.on_set_status.call((press.card, status.as_str().to_owned()));
                    }
                }
            },
            div { class: "flex items-center gap-2 px-1 py-1",
                span { class: "h-2 w-2 shrink-0 rounded-full {header_dot}" }
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
    let lifted = drag.card().is_some_and(|p| p.card == id);
    rsx! {
        // `items-stretch justify-start` spelled out: a renderer's button
        // defaults centre a flex button's content (Blitz does).
        button {
            r#type: "button",
            class: if lifted {
                "flex cursor-grabbing flex-col items-stretch justify-start gap-1.5 rounded-md border border-dashed border-border bg-card p-2.5 text-left opacity-40"
            } else {
                "flex cursor-grab flex-col items-stretch justify-start gap-1.5 rounded-md border border-border bg-card p-2.5 text-left shadow-sm hover:border-primary/50 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            },
            // The board's keyboard model treats a card as a row.
            "data-task-row": "{id}",
            "data-row-focus": "true",
            "data-board-card": "true",
            aria_keyshortcuts: "Shift+ArrowLeft Shift+ArrowRight",
            onpointerdown: move |e: PointerEvent| {
                drag.swallow_click.set(false);
                // Touch scrolls the board; it moves cards from the sheet.
                if e.data().pointer_type() == "touch" {
                    return;
                }
                let at = e.data().client_coordinates();
                drag.press.set(Some(Press { card: id, from: status, x: at.x, y: at.y }));
            },
            onclick: move |_| {
                if *drag.swallow_click.peek() {
                    drag.swallow_click.set(false);
                    return;
                }
                props.on_open.call(id);
            },
            span { class: "text-sm text-foreground break-words", "{t.title}" }
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
