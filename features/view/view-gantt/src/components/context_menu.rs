//! Right-click context menu over a bar.
//!
//! Implemented as a position-fixed overlay opened by setting
//! [`ContextMenuState::open`] — bars do that on their
//! `oncontextmenu` handler. architect-ui doesn't expose a primitive that
//! survives `position: absolute` parents without breaking our chart
//! coordinates, so the menu is a small bespoke widget.

use dioxus::prelude::*;
use architect_ui::prelude::*;

use crate::store::GanttEvent;
use crate::types::{TaskId, TaskType};

use super::gantt::GanttContext;

#[derive(Clone, Copy, Debug)]
pub struct ContextMenuTarget {
    pub x: f32,
    pub y: f32,
    pub task_id: TaskId,
}

#[derive(Clone, Copy)]
pub struct ContextMenuContext {
    pub open: Signal<Option<ContextMenuTarget>>,
}

pub fn use_context_menu() -> ContextMenuContext {
    use_context::<ContextMenuContext>()
}

#[component]
pub fn ContextMenuOverlay() -> Element {
    let mut menu = use_context_menu().open;
    let ctx = use_context::<GanttContext>();
    let on_event = ctx.on_event.clone();
    let state = ctx.state;

    let Some(target) = *menu.read() else {
        return rsx!(div {});
    };

    let task = state
        .read()
        .tasks
        .iter()
        .find(|t| t.id == target.task_id)
        .cloned();
    let Some(task) = task else {
        menu.set(None);
        return rsx!(div {});
    };

    let readonly = state.read().readonly;
    let id = target.task_id;
    let x = target.x;
    let y = target.y;

    // Backdrop click closes; clicks on the menu itself stop
    // propagation so the menu stays open until a real action.
    let mut close = move || menu.set(None);

    let on_edit = {
        let on_event = on_event.clone();
        move |()| {
            on_event.call(GanttEvent::OpenEditor { id });
            close();
        }
    };
    let on_delete = {
        let on_event = on_event.clone();
        move |()| {
            on_event.call(GanttEvent::DeleteTask { id });
            close();
        }
    };
    let on_convert = {
        let on_event = on_event.clone();
        let current = task.task_type;
        move |new_type: TaskType| {
            on_event.call(GanttEvent::UpdateType {
                id,
                task_type: new_type,
            });
            let _ = current;
            close();
        }
    };
    let on_dup = {
        let on_event = on_event.clone();
        let mut task_clone = task.clone();
        move |()| {
            task_clone.id = uuid::Uuid::new_v4();
            task_clone.text = format!("{} (copy)", task_clone.text);
            on_event.call(GanttEvent::AddTask {
                task: task_clone.clone(),
            });
            close();
        }
    };

    rsx! {
        // Full-viewport backdrop catches the dismissal click without
        // dimming anything (transparent).
        div {
            class: "fixed inset-0 z-50",
            onclick: move |_| close(),
            oncontextmenu: move |e: Event<MouseData>| {
                e.prevent_default();
                close();
            },
            div {
                class: "absolute min-w-48 rounded-lg bg-popover text-popover-foreground border border-border shadow-md p-1",
                style: "left: {x}px; top: {y}px;",
                onclick: move |e: Event<MouseData>| e.stop_propagation(),
                MenuItem {
                    label: "Edit details",
                    on_click: EventHandler::new(on_edit),
                }
                MenuItem {
                    label: "Duplicate",
                    disabled: readonly,
                    on_click: EventHandler::new(on_dup),
                }
                MenuSeparator {}
                MenuItem {
                    label: if matches!(task.task_type, TaskType::Task) {
                        "Convert to milestone"
                    } else {
                        "Convert to task"
                    },
                    disabled: readonly || matches!(task.task_type, TaskType::Summary),
                    on_click: {
                        let mut on_convert = on_convert.clone();
                        EventHandler::new(move |()| {
                            let new = if matches!(task.task_type, TaskType::Task) {
                                TaskType::Milestone
                            } else {
                                TaskType::Task
                            };
                            on_convert(new);
                        })
                    },
                }
                MenuSeparator {}
                MenuItem {
                    label: "Delete",
                    destructive: true,
                    disabled: readonly,
                    on_click: EventHandler::new(on_delete),
                }
            }
        }
    }
}

#[derive(Props, PartialEq, Clone)]
struct MenuItemProps {
    label: String,
    #[props(default = false)]
    destructive: bool,
    #[props(default = false)]
    disabled: bool,
    on_click: EventHandler<()>,
}

#[component]
fn MenuItem(props: MenuItemProps) -> Element {
    let base = "flex w-full items-center rounded-md px-3 py-2 text-sm transition-colors text-left";
    let interactive = if props.disabled {
        "opacity-50 cursor-not-allowed"
    } else if props.destructive {
        "text-destructive hover:bg-destructive/10 cursor-pointer"
    } else {
        "hover:bg-accent hover:text-accent-foreground cursor-pointer"
    };
    rsx! {
        button {
            class: "{base} {interactive}",
            disabled: props.disabled,
            onclick: move |_| {
                if !props.disabled {
                    props.on_click.call(());
                }
            },
            "{props.label}"
        }
    }
}

#[component]
fn MenuSeparator() -> Element {
    rsx! { div { class: "bg-border/50 my-1 h-px" } }
}

// Keep the import-tracker happy.
#[allow(unused)]
fn _force_imports() {
    let _ = Spacer;
}
