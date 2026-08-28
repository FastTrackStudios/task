//! Live activity — open timers, in-flight agents, and the active
//! slice of the task board.

use agent_proto::session::{Session as AgentSession, SessionStatus as AgentStatus};
use architect_ui::prelude::*;
use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use task_proto::TaskInfo as DbTask;
use timer_proto::WorkSession;
use uuid::Uuid;

use crate::routes::Route;

use super::status_variant;

/// Open timers + in-flight agent sessions. `snapshot == None` while
/// the first fetch is in flight (quiet placeholder, no blanking);
/// empty lists render the nobody-active state.
#[component]
pub(super) fn ActiveNowSection(
    snapshot: Option<(Vec<WorkSession>, Vec<AgentSession>)>,
    you: Option<Uuid>,
) -> Element {
    // Silence when nobody's here: a permanent "Nobody's working on
    // this" card is furniture, not information. The section only
    // exists while there is activity to show.
    match &snapshot {
        None => return rsx! {},
        Some((timers, agents)) if timers.is_empty() && agents.is_empty() => {
            return rsx! {};
        }
        Some(_) => {}
    }
    rsx! {
        div { class: "flex flex-col gap-2",
            span { class: "text-[11px] font-semibold uppercase tracking-wider text-muted-foreground",
                "Active now"
            }
            match &snapshot {
                None => rsx! {},
                Some((timers, agents)) if timers.is_empty() && agents.is_empty() => rsx! {},
                Some((timers, agents)) => rsx! {
                    div { class: "flex flex-col divide-y divide-border/50 rounded-xl border border-border/60 bg-card/40",
                        for s in timers.iter() {
                            TimerRow { key: "{s.id}", session: s.clone(), you }
                        }
                        for s in agents.iter() {
                            AgentRow { key: "{s.id}", session: s.clone() }
                        }
                    }
                },
            }
        }
    }
}

/// One open timer session: who, what, since when, on which task.
#[component]
fn TimerRow(session: WorkSession, you: Option<Uuid>) -> Element {
    let who = if Some(session.user_id) == you {
        "you".to_string()
    } else {
        // No member directory yet — fall back to a short stable id.
        session.user_id.to_string()[..8].to_string()
    };
    let what = if session.description.trim().is_empty() {
        "(no description)".to_string()
    } else {
        session.description.clone()
    };
    let since = ago_label(Utc::now(), session.start_time);
    rsx! {
        div { class: "flex items-center justify-between gap-3 px-3 py-2.5",
            div { class: "flex min-w-0 flex-col gap-0.5",
                div { class: "flex items-center gap-2 text-sm",
                    span { class: "font-medium", "{who}" }
                    span { class: "truncate text-muted-foreground", "{what}" }
                }
                div { class: "flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground",
                    span { "started {since}" }
                    if !session.task_note_path.is_empty() {
                        span { "·" }
                        span { class: "truncate font-mono", "{session.task_note_path}" }
                    }
                }
            }
            StatusBadge { variant: StatusBadgeVariant::Success, label: "timer".to_string() }
        }
    }
}

/// One in-flight agent session: nickname / title, status, what it's
/// chewing on, last activity.
#[component]
fn AgentRow(session: AgentSession) -> Element {
    let name = if !session.subagent_nickname.is_empty() {
        session.subagent_nickname.clone()
    } else if !session.title.trim().is_empty() {
        session.title.clone()
    } else {
        "(untitled agent)".to_string()
    };
    let turn = session
        .pending
        .as_ref()
        .map(|pt| turn_summary(&pt.user_message));
    let last = session.last_message_at.map(|t| ago_label(Utc::now(), t));
    rsx! {
        div { class: "flex items-center justify-between gap-3 px-3 py-2.5",
            div { class: "flex min-w-0 flex-col gap-0.5",
                span { class: "truncate text-sm font-medium", "{name}" }
                div { class: "flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground",
                    if let Some(t) = turn {
                        span { class: "truncate", "{t}" }
                    }
                    if let Some(l) = last {
                        span { "·" }
                        span { "{l}" }
                    }
                }
            }
            StatusBadge {
                variant: agent_status_variant(session.status),
                label: agent_status_label(session.status).to_string(),
            }
        }
    }
}

fn agent_status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "Idle",
        AgentStatus::Running => "Running",
        AgentStatus::AwaitingUser => "Awaiting user",
        AgentStatus::Cancelled => "Cancelled",
        AgentStatus::Errored => "Errored",
    }
}

fn agent_status_variant(status: AgentStatus) -> StatusBadgeVariant {
    match status {
        AgentStatus::Running => StatusBadgeVariant::Success,
        AgentStatus::AwaitingUser => StatusBadgeVariant::Warning,
        AgentStatus::Errored => StatusBadgeVariant::Danger,
        AgentStatus::Idle | AgentStatus::Cancelled => StatusBadgeVariant::Neutral,
    }
}

/// First line of the pending user message, clipped — the "what is it
/// doing" summary in the agent row.
fn turn_summary(msg: &str) -> String {
    let line = msg.lines().next().unwrap_or_default().trim();
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        out.push('…');
    }
    out
}

/// Compact relative time ("just now" / "5m ago" / "3h ago" / "2d
/// ago"). Pure (injected `now`) so it's testable.
fn ago_label(now: DateTime<Utc>, t: DateTime<Utc>) -> String {
    let secs = (now - t).num_seconds().max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{}m ago", secs / 60),
        3_600..=86_399 => format!("{}h ago", secs / 3_600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// 30-second poll cadence for the active-now / budget refresh. Native
/// parks (no poll) — same pattern as the chrome's second tick.
#[cfg(target_arch = "wasm32")]
pub(super) async fn sleep_30s() {
    gloo_timers::future::TimeoutFuture::new(30_000).await;
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn sleep_30s() {
    futures_util::future::pending::<()>().await;
}

/// Comma-joined holder labels from `workflow.assignees`.
fn assignee_labels(t: &DbTask) -> String {
    use task_proto::workflows_proto::AgentRef;
    let Some(w) = &t.workflow else {
        return String::new();
    };
    w.assignees
        .0
        .iter()
        .map(|a| match a {
            AgentRef::Human {
                user_id,
                display_name,
            } => display_name.clone().unwrap_or_else(|| user_id.clone()),
            AgentRef::Agent { name, .. } => format!("agent:{name}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// One row in the "Active" slice: title (links to the task detail),
/// holders, status badge.
#[component]
pub(super) fn ActiveTaskRow(task: ReadSignal<DbTask>) -> Element {
    let task = task.read();
    let holders = assignee_labels(&task);
    rsx! {
        div { class: "flex items-center justify-between gap-3 py-1 text-sm",
            Link {
                to: Route::TaskDetailRoute { id: task.id },
                class: "min-w-0 truncate font-medium hover:underline",
                "{task.title}"
            }
            div { class: "flex shrink-0 items-center gap-2",
                if !holders.is_empty() {
                    span { class: "text-xs text-muted-foreground", "{holders}" }
                }
                StatusBadge { variant: status_variant(&task.status), label: task.status.clone() }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_label_buckets() {
        let now = Utc::now();
        let s = |secs: i64| now - chrono::Duration::seconds(secs);
        assert_eq!(ago_label(now, s(10)), "just now");
        assert_eq!(ago_label(now, s(90)), "1m ago");
        assert_eq!(ago_label(now, s(2 * 3_600 + 60)), "2h ago");
        assert_eq!(ago_label(now, s(3 * 86_400)), "3d ago");
        // Clock skew (event in the future) clamps to "just now".
        assert_eq!(
            ago_label(now, now + chrono::Duration::seconds(30)),
            "just now"
        );
    }

    #[test]
    fn turn_summary_first_line_clipped() {
        assert_eq!(
            turn_summary("fix the build\nand then more"),
            "fix the build"
        );
        let long = "x".repeat(120);
        let clipped = turn_summary(&long);
        assert_eq!(clipped.chars().count(), 81); // 80 + ellipsis
        assert!(clipped.ends_with('…'));
        assert_eq!(turn_summary(""), "");
    }
}
