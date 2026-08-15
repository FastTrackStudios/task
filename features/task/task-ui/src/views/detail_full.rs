//! `TaskDetailFull` — the rich, read-mostly detail surface.
//!
//! Designed to back BOTH the `/tasks/:id` page and the existing
//! quick-edit sheet. Like every other view in this crate it
//! renders `task_proto::TaskInfo` + `WorkflowAttrs` directly —
//! the page layer already holds the authoritative
//! `Vec<TaskInfo>` (see `crates/ui`'s `task_wiring`), so no
//! mirror or converter sits in between.
//!
//! Doctrine (plans/project-overview-task-ui.md): all data in via
//! props (pre-resolved labels included — no lookups here), all
//! intent out via callbacks. RPC wiring (`try_claim`, estimate
//! writes, session fetch) happens in the page layer later.

use dioxus::prelude::*;
use task_proto::model::Estimate;
use task_proto::{Priority, Status, TaskInfo as DbTask};
use uuid::Uuid;
use workflows_proto::{Activity, Transition};

use crate::display::{PriorityLabel, StatusLabel};
use crate::views::links::LinkChips;
use crate::views::palette::{priority_pill, status_pill};
use crate::views::session_history::SessionHistory;
use crate::views::subtasks::SubtasksBoard;
use crate::views::time::TimeSection;
use crate::views::workflow::WorkflowSection;

// ---------------------------------------------------------------------------
// Shared prop shapes
// ---------------------------------------------------------------------------

/// A task reference with a pre-resolved title. The page layer
/// resolves ids → titles; anything it couldn't resolve renders
/// as a short-id chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedTaskRef {
    pub id: Uuid,
    pub title: Option<String>,
}

/// One subtask row: the record + who currently claims it
/// (first assignee, resolved by the page layer).
#[derive(Clone, Debug, PartialEq)]
pub struct SubtaskRow {
    pub task: DbTask,
    pub claimant: Option<workflows_proto::AgentRef>,
}

/// Current claim state of the viewed task, as understood by the
/// page layer. Mirrors `task_proto::service::ClaimResult` so wiring is
/// a `From` away; `Pending` covers the in-flight RPC.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ClaimState {
    /// No assignee — the claim button is live.
    #[default]
    Unclaimed,
    /// The viewing actor holds the claim.
    Mine,
    /// Someone else holds it.
    Held { holder: String },
    /// A claim RPC is in flight.
    Pending,
}

impl From<task_proto::service::ClaimResult> for ClaimState {
    fn from(r: task_proto::service::ClaimResult) -> Self {
        match r {
            task_proto::service::ClaimResult::Won
            | task_proto::service::ClaimResult::AlreadyMine => Self::Mine,
            task_proto::service::ClaimResult::Lost { holder } => Self::Held { holder },
        }
    }
}

/// First 8 hex chars of a uuid — the chip fallback when no
/// title was resolved.
#[must_use]
pub fn short_id(id: &Uuid) -> String {
    id.simple().to_string()[..8].to_string()
}

/// Resolve a list of raw ids against pre-resolved refs, keeping
/// the id-list order. Ids the page layer didn't resolve come
/// back with `title: None` (rendered as short-id chips).
#[must_use]
pub fn resolve_links(ids: &[Uuid], resolved: &[LinkedTaskRef]) -> Vec<LinkedTaskRef> {
    ids.iter()
        .map(|id| LinkedTaskRef {
            id: *id,
            title: resolved
                .iter()
                .find(|r| r.id == *id)
                .and_then(|r| r.title.clone()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct TaskDetailFullProps {
    /// Full persistence record (`task_proto::TaskInfo`).
    pub task: DbTask,
    /// Resolved display label for `workflow.cycle`. Display-only
    /// this phase; `None` with a cycle id set falls back to the
    /// short id.
    #[props(default)]
    pub cycle_label: Option<String>,
    /// Subtask rows (tasks whose `workflow.parent` is this task),
    /// each with its resolved claimant.
    #[props(default)]
    pub subtasks: Vec<SubtaskRow>,
    /// Pre-resolved titles for `workflow.blockers` /
    /// `workflow.relates_to`. Partial is fine — unresolved ids
    /// render as short-id chips.
    #[props(default)]
    pub resolved_blockers: Vec<LinkedTaskRef>,
    #[props(default)]
    pub resolved_relates_to: Vec<LinkedTaskRef>,
    /// Session audit records for `workflow.session`, fetched by
    /// the page layer. Rendered chronologically (UUIDv7 order).
    #[props(default)]
    pub transitions: Vec<Transition>,
    #[props(default)]
    pub activities: Vec<Activity>,
    /// Claim state as resolved by the page layer.
    #[props(default)]
    pub claim: ClaimState,
    /// Estimate chip clicked — `None` clears. The page layer
    /// writes it through (`workflow.estimate`) and re-renders.
    pub on_set_estimate: EventHandler<Option<Estimate>>,
    /// Claim button pressed. The page layer calls `try_claim`
    /// and maps the `ClaimResult` back into `claim`.
    pub on_claim: EventHandler<()>,
    /// A subtask row / blocker chip / relates-to chip was
    /// clicked — navigate to that task.
    pub on_open: EventHandler<Uuid>,
}

/// Full detail surface: header, markdown body, workflow,
/// subtasks, links, time, session history. Read-mostly — the
/// only interactive bits are the estimate picker, the claim
/// button, and cross-task navigation.
#[component]
pub fn TaskDetailFull(props: TaskDetailFullProps) -> Element {
    let t = &props.task;
    let wf = t.workflow.clone().unwrap_or_default();
    let status = Status::from_str(&t.status);
    let priority = Priority::from_str(&t.priority).unwrap_or(Priority::Normal);

    let blockers = resolve_links(&wf.blockers, &props.resolved_blockers);
    let relates = resolve_links(&wf.relates_to, &props.resolved_relates_to);

    let has_time =
        t.time_estimate.is_some() || !t.time_entries.is_empty() || t.recurrence.is_some();
    let has_history = !props.transitions.is_empty() || !props.activities.is_empty();

    rsx! {
        article { class: "flex flex-col gap-5",
            // ---- Header -------------------------------------------------
            header { class: "flex flex-col gap-2",
                h1 { class: "text-xl font-semibold text-foreground leading-tight break-words", "{t.title}" }
                div { class: "flex flex-wrap items-center gap-1.5",
                    {
                        let (label, cls) = match status {
                            Some(s) => (s.label().to_string(), status_pill(s)),
                            None => (t.status.clone(), "bg-muted/50 text-muted-foreground border border-border"),
                        };
                        rsx! {
                            span { class: "inline-flex items-center rounded-full px-2 py-0.5 text-[11px] uppercase tracking-wider {cls}",
                                "{label}"
                            }
                        }
                    }
                    if priority != Priority::Normal {
                        span { class: "inline-flex items-center rounded-full px-2 py-0.5 text-[11px] uppercase tracking-wider {priority_pill(priority)}",
                            "{priority.label()}"
                        }
                    }
                    if let Some(due) = t.due.as_deref() {
                        span { class: "inline-flex items-center rounded-full border border-border px-2 py-0.5 text-[11px] text-muted-foreground",
                            "Due {due}"
                        }
                    }
                    span { class: "ml-auto font-mono text-[10px] text-muted-foreground/70",
                        "{short_id(&t.id)}"
                    }
                }
            }

            // ---- Workflow (estimate / cycle / assignees / claim) --------
            WorkflowSection {
                estimate: wf.estimate,
                cycle_id: wf.cycle,
                cycle_label: props.cycle_label.clone(),
                assignees: wf.assignees.0.clone(),
                claim: props.claim.clone(),
                on_set_estimate: props.on_set_estimate,
                on_claim: props.on_claim,
            }

            // ---- PRD body ----------------------------------------------
            if !t.details.trim().is_empty() {
                section { class: "flex flex-col gap-2",
                    SectionLabel { label: "Details" }
                    crate::markdown::Markdown { source: t.details.clone() }
                }
            }

            // ---- Subtasks ----------------------------------------------
            if !props.subtasks.is_empty() {
                SubtasksBoard { rows: props.subtasks.clone(), on_open: props.on_open }
            }

            // ---- Blockers / relates-to ---------------------------------
            if !blockers.is_empty() {
                LinkChips {
                    label: "Blocked by",
                    links: blockers,
                    on_open: props.on_open,
                }
            }
            if !relates.is_empty() {
                LinkChips {
                    label: "Relates to",
                    links: relates,
                    on_open: props.on_open,
                }
            }

            // ---- Time / recurrence -------------------------------------
            if has_time {
                TimeSection {
                    time_estimate: t.time_estimate,
                    entries: t.time_entries.0.clone(),
                    recurrence: t.recurrence.clone(),
                    recurrence_anchor: t.recurrence_anchor.clone(),
                    complete_instances: t.complete_instances.0.clone(),
                }
            }

            // ---- Session history ---------------------------------------
            if has_history {
                SessionHistory {
                    transitions: props.transitions.clone(),
                    activities: props.activities.clone(),
                }
            }
        }
    }
}

/// Tiny uppercase section label — shared style across the
/// detail sections.
#[component]
pub(crate) fn SectionLabel(label: &'static str) -> Element {
    rsx! {
        span { class: "text-[10px] uppercase tracking-wider text-muted-foreground", "{label}" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_is_first_eight_hex() {
        let id = Uuid::parse_str("dcba34cd-0000-4000-8000-000000000000").unwrap();
        assert_eq!(short_id(&id), "dcba34cd");
    }

    #[test]
    fn resolve_links_keeps_order_and_marks_missing() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let resolved = vec![LinkedTaskRef {
            id: b,
            title: Some("Known".into()),
        }];
        let out = resolve_links(&[a, b], &resolved);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, a);
        assert_eq!(out[0].title, None);
        assert_eq!(out[1].title.as_deref(), Some("Known"));
    }

    #[test]
    fn claim_state_from_claim_result() {
        use task_proto::service::ClaimResult;
        assert_eq!(ClaimState::from(ClaimResult::Won), ClaimState::Mine);
        assert_eq!(ClaimState::from(ClaimResult::AlreadyMine), ClaimState::Mine);
        assert_eq!(
            ClaimState::from(ClaimResult::Lost {
                holder: "claude@fable-5".into()
            }),
            ClaimState::Held {
                holder: "claude@fable-5".into()
            }
        );
    }
}
