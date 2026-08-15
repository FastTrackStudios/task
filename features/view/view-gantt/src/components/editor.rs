//! Side-sheet task editor (architect-ui [`Sheet`]).
//!
//! Bound to whatever task is in `state.editing`. Closes via Sheet's
//! `on_close` or the explicit Close button. Mutations dispatched as
//! [`GanttEvent`] so consumers see the same event stream as drag /
//! resize.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use dioxus::prelude::*;
use architect_ui::prelude::*;

use crate::store::GanttEvent;
use crate::types::{TaskId, TaskType};

fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Utc.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).single()
}

use super::gantt::GanttContext;

#[derive(Props, PartialEq, Clone)]
pub struct TaskEditorProps {
    pub task_id: TaskId,
}

#[component]
pub fn TaskEditor(props: TaskEditorProps) -> Element {
    let ctx = use_context::<GanttContext>();
    let state = ctx.state;
    let on_event = ctx.on_event.clone();

    let snapshot = state
        .read()
        .tasks
        .iter()
        .find(|t| t.id == props.task_id)
        .cloned();
    let Some(task) = snapshot else {
        return rsx!(div {});
    };

    let id = props.task_id;
    let close_event = on_event.clone();
    let close_event2 = on_event.clone();
    let close_event3 = on_event.clone();

    let start_str = task.start.format("%Y-%m-%d").to_string();
    let end_str = task.end.format("%Y-%m-%d").to_string();
    let progress_pct = (task.progress.clamp(0.0, 1.0) * 100.0).round() as i32;
    let type_value = match task.task_type {
        TaskType::Task => "task",
        TaskType::Summary => "summary",
        TaskType::Milestone => "milestone",
    }
    .to_string();

    let on_text = {
        let on_event = on_event.clone();
        move |e: FormEvent| {
            on_event.call(GanttEvent::UpdateText {
                id,
                text: e.value(),
            });
        }
    };

    let on_progress = {
        let on_event = on_event.clone();
        move |e: FormEvent| {
            if let Ok(v) = e.value().parse::<f32>() {
                on_event.call(GanttEvent::UpdateProgress {
                    id,
                    progress: v / 100.0,
                });
            }
        }
    };

    let on_type = {
        let on_event = on_event.clone();
        Callback::new(move |v: String| {
            let task_type = match v.as_str() {
                "summary" => TaskType::Summary,
                "milestone" => TaskType::Milestone,
                _ => TaskType::Task,
            };
            on_event.call(GanttEvent::UpdateType { id, task_type });
        })
    };

    rsx! {
        Sheet {
            open: true,
            on_close: move |()| close_event.call(GanttEvent::CloseEditor),
            side: SheetSide::Right,
            SheetHeader {
                SheetTitle { "Task details" }
                SheetDescription { "Edit name, type, progress." }
            }
            div { class: "flex flex-col gap-4",
                Field {
                    FieldLabel { "Name" }
                    input {
                        class: "h-9 w-full rounded-md border border-input bg-background px-3 text-sm",
                        value: "{task.text}",
                        oninput: on_text,
                    }
                }
                Field {
                    FieldLabel { "Type" }
                    SegmentedControl {
                        value: type_value.clone(),
                        options: vec![
                            ("task".into(), "Task".into()),
                            ("summary".into(), "Summary".into()),
                            ("milestone".into(), "Milestone".into()),
                        ],
                        on_change: on_type,
                    }
                }
                Field {
                    FieldLabel { "Progress" }
                    input {
                        r#type: "range",
                        min: "0",
                        max: "100",
                        step: "1",
                        value: "{progress_pct}",
                        oninput: on_progress,
                        class: "w-full",
                    }
                    Text { variant: TextVariant::Muted, "{progress_pct}%" }
                }
                Field {
                    FieldLabel { "Start" }
                    input {
                        r#type: "date",
                        class: "h-9 w-full rounded-md border border-input bg-background px-3 text-sm",
                        value: "{start_str}",
                        onchange: {
                            let on_event = on_event.clone();
                            let task_end = task.end;
                            move |e: FormEvent| {
                                if let Some(d) = parse_date(&e.value()) {
                                    let new_start = d.min(task_end - chrono::Duration::hours(1));
                                    on_event.call(GanttEvent::UpdateDates {
                                        id,
                                        start: new_start,
                                        end: task_end,
                                    });
                                }
                            }
                        },
                    }
                }
                Field {
                    FieldLabel { "End" }
                    input {
                        r#type: "date",
                        class: "h-9 w-full rounded-md border border-input bg-background px-3 text-sm",
                        value: "{end_str}",
                        onchange: {
                            let on_event = on_event.clone();
                            let task_start = task.start;
                            move |e: FormEvent| {
                                if let Some(d) = parse_date(&e.value()) {
                                    let new_end = d.max(task_start + chrono::Duration::hours(1));
                                    on_event.call(GanttEvent::UpdateDates {
                                        id,
                                        start: task_start,
                                        end: new_end,
                                    });
                                }
                            }
                        },
                    }
                }
                if let Some(d) = &task.details {
                    Field {
                        FieldLabel { "Details" }
                        Text { variant: TextVariant::Muted, "{d}" }
                    }
                }
            }
            SheetFooter {
                Button {
                    variant: ButtonVariant::Secondary,
                    on_click: move |_| close_event2.call(GanttEvent::CloseEditor),
                    "Close"
                }
                Button {
                    variant: ButtonVariant::Destructive,
                    on_click: move |_| {
                        close_event3.call(GanttEvent::DeleteTask { id });
                        close_event3.call(GanttEvent::CloseEditor);
                    },
                    "Delete"
                }
            }
        }
    }
}
