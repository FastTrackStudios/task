//! Canonical task/project sort keys + membership predicates.
//!
//! These used to live as per-page copies in `home.rs`, `projects.rs`,
//! and `project_detail.rs` — and the copies had drifted: `home`'s
//! `priority_rank` treated `"critical"` as urgent (rank 0) while
//! `projects`' copy fell through to the unknown bucket (rank 4), so
//! the same task sorted differently on the two pages. Dedupe here so
//! every list agrees on one ordering. (Issue #27, item 2.)

use project_proto::ProjectInfo;
use task_proto::TaskInfo as DbTask;

/// Sort key for task/issue priority: urgent → high → normal → low →
/// lowest/unknown. Lower rank sorts first. Empty string is treated as
/// `normal` (the unset default).
pub fn priority_rank(priority: &str) -> u8 {
    match priority {
        "p0" | "urgent" | "critical" => 0,
        "p1" | "high" => 1,
        "p2" | "normal" | "" => 2,
        "p3" | "low" => 3,
        _ => 4,
    }
}

/// Sort key for project/workflow status: active first, paused next,
/// in-progress/other, then done, then cancelled. Lower rank sorts
/// first.
pub fn status_rank(status: &str) -> u8 {
    match status {
        "active" | "open" | "in_progress" => 0,
        "on_hold" | "on-hold" | "paused" | "waiting" => 1,
        "done" | "completed" | "shipped" => 3,
        "cancelled" | "canceled" | "abandoned" => 4,
        _ => 2,
    }
}

/// Whether a status string is in the "active" bucket (the same set
/// [`status_rank`] ranks at 0).
pub fn is_active(status: &str) -> bool {
    matches!(status, "active" | "open" | "in_progress")
}

/// Whether a task is still open (not in any terminal status).
pub fn is_open_task(t: &DbTask) -> bool {
    task_proto::status_is_open(&t.status)
}

/// Whether a task is actively being worked: explicitly `in-progress`,
/// or claimed by someone via `workflow.assignees`.
pub fn is_active_task(t: &DbTask) -> bool {
    t.status == "in-progress"
        || t.workflow
            .as_ref()
            .is_some_and(|w| !w.assignees.0.is_empty())
}

/// A task belongs to a project by `project_id`, or by a `[[title]]` /
/// bare-title entry in its `projects:` wikilink list.
pub fn belongs(t: &DbTask, p: &ProjectInfo) -> bool {
    if t.project_id == Some(p.id) {
        return true;
    }
    let link = format!("[[{}]]", p.title);
    t.projects.0.iter().any(|x| x == &link || x == &p.title)
}
