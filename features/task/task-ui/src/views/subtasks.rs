//! Subtasks board — the triage workflow breaks an issue into N
//! agent-sized subtasks (`workflow.parent` = this task). Rows
//! show status + claimant; the header shows done/total.

use dioxus::prelude::*;
use uuid::Uuid;

use task_proto::Status;

use crate::display::StatusLabel;
use crate::views::detail_full::{SectionLabel, SubtaskRow};
use crate::views::palette::status_pill;

/// `(done, total)` across the rows. "Done" = the status
/// classifies into the `completed` state group (group-routed,
/// so custom status names count too).
#[must_use]
pub fn subtask_summary(rows: &[SubtaskRow]) -> (usize, usize) {
    let done = rows
        .iter()
        .filter(|r| {
            project_proto::resolve_state_group(None, &r.task.status) == project_proto::StateGroup::Completed
        })
        .count();
    (done, rows.len())
}

#[derive(Props, Clone, PartialEq)]
pub struct SubtasksBoardProps {
    pub rows: Vec<SubtaskRow>,
    /// Row clicked — navigate to that subtask.
    pub on_open: EventHandler<Uuid>,
}

#[component]
pub fn SubtasksBoard(props: SubtasksBoardProps) -> Element {
    let (done, total) = subtask_summary(&props.rows);
    rsx! {
        section { class: "flex flex-col gap-2",
            div { class: "flex items-center gap-2",
                SectionLabel { label: "Subtasks" }
                span { class: "text-[11px] text-muted-foreground", "{done}/{total} done" }
            }
            div { class: "flex flex-col rounded-lg border border-border divide-y divide-border overflow-hidden",
                for row in props.rows.iter() {
                    {
                        let id = row.task.id;
                        let status = Status::from_str(&row.task.status);
                        // Strike-through follows the state group
                        // (progress decision), the pill keeps the
                        // raw status / enum label (display).
                        let done_row = project_proto::resolve_state_group(None, &row.task.status)
                            == project_proto::StateGroup::Completed;
                        let title_cls = if done_row {
                            "flex-1 min-w-0 truncate text-sm text-muted-foreground line-through"
                        } else {
                            "flex-1 min-w-0 truncate text-sm text-foreground"
                        };
                        let (status_label, status_cls) = match status {
                            Some(s) => (s.label().to_string(), status_pill(s)),
                            None => (
                                row.task.status.clone(),
                                "bg-muted/50 text-muted-foreground border border-border",
                            ),
                        };
                        rsx! {
                            button {
                                key: "{id}",
                                r#type: "button",
                                class: "flex w-full items-center gap-2 bg-card/30 px-3 py-2 text-left hover:bg-accent/30",
                                onclick: move |_| props.on_open.call(id),
                                span { class: "inline-flex shrink-0 items-center rounded-full px-1.5 py-0 text-[10px] uppercase tracking-wider {status_cls}",
                                    "{status_label}"
                                }
                                span { class: "{title_cls}", "{row.task.title}" }
                                if let Some(claimant) = &row.claimant {
                                    span { class: "shrink-0 rounded-full bg-muted/50 px-1.5 py-0 text-[10px] text-muted-foreground",
                                        "{claimant.short_label()}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str) -> SubtaskRow {
        let mut t = task_proto::capture("sub");
        t.status = status.to_string();
        SubtaskRow {
            task: t,
            claimant: None,
        }
    }

    #[test]
    fn summary_counts_done_rows() {
        let rows = vec![row("done"), row("open"), row("completed"), row("waiting")];
        assert_eq!(subtask_summary(&rows), (2, 4));
    }

    #[test]
    fn unknown_statuses_count_as_open() {
        let rows = vec![row("qa-review")];
        assert_eq!(subtask_summary(&rows), (0, 1));
    }
}
