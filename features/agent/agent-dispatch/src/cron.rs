//! Slice 4 — daily cron + weekly digest.
//!
//! - [`schedule_recurring`] walks the vault for task notes
//!   that are (a) agent-runnable (`agent_profile` set) and
//!   (b) scheduled for today or earlier; dispatches each as a
//!   fresh agent task in the queue named by `project` (or
//!   `"default"`).
//! - [`weekly_digest`] reads archived/done agent tasks for the
//!   target week and emits a markdown blob the caller can paste
//!   into a project note.

use std::path::Path;

use agent_proto::tasks::{AgentTask, status};
use agent_tasks::entity::{AgentTaskColumn, AgentTaskEntity};
use chrono::{Datelike, Duration, NaiveDate, Utc};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use task::parse_str;

use crate::error::DispatchError;
use crate::ops::{DispatchOptions, DispatchOutcome, dispatch};

/// Report from one cron tick.
#[derive(Debug, Default)]
pub struct ScheduleReport {
    /// Successfully dispatched. One entry per source task note.
    pub dispatched: Vec<DispatchOutcome>,
    /// Skipped — file matched the agent-runnable shape but
    /// dispatch failed. Caller logs + moves on.
    pub failures: Vec<(String, String)>,
    /// Files inspected.
    pub inspected: u32,
}

/// Walk `<vault_root>/tasks/*.md` and dispatch every task note
/// that has an `agent_profile` set AND a `scheduled:` date in
/// the past (or no `scheduled:` at all, for non-recurring
/// agent-runnable tasks). Idempotent within the same `today`:
/// won't double-dispatch a task that already has an
/// `agent_task` in a non-terminal status whose `source_task`
/// matches.
pub async fn schedule_recurring(
    store: &agent_tasks::Store,
    vault_root: &Path,
    today: NaiveDate,
) -> Result<ScheduleReport, DispatchError> {
    let tasks_dir = vault_root.join("tasks");
    let mut report = ScheduleReport::default();
    if !tasks_dir.is_dir() {
        return Ok(report);
    }
    let dir_iter = std::fs::read_dir(&tasks_dir)?;
    for entry in dir_iter.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        report.inspected += 1;
        let rel_path = format!(
            "tasks/{}",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("")
        );
        let basename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => {
                report.failures.push((rel_path, format!("read: {e}")));
                continue;
            }
        };
        let mut t = match parse_str(&rel_path, basename, &raw) {
            Ok(t) => t,
            Err(e) => {
                report.failures.push((rel_path, format!("parse: {e}")));
                continue;
            }
        };
        if t.agent_profile.is_empty() {
            continue;
        }
        if !is_due(t.scheduled.as_deref(), today) {
            continue;
        }
        // Idempotence: if any non-archived agent task already
        // exists for this source_task in a non-terminal state,
        // skip. (Done/archived for prior days don't count —
        // they're history.)
        let existing = AgentTaskEntity::find()
            .filter(AgentTaskColumn::SourceTask.eq(t.path.clone()))
            .filter(AgentTaskColumn::Status.ne(status::DONE))
            .filter(AgentTaskColumn::Status.ne(status::ARCHIVED))
            .one(store.conn())
            .await
            .map_err(|e| DispatchError::Agent(agent_proto::AgentError::Backend(e.to_string())))?;
        if existing.is_some() {
            continue;
        }

        let queue_id = if let Some(p) = t.projects.first() {
            project_link_to_slug(p)
        } else {
            "default".into()
        };
        let opts = DispatchOptions {
            queue_id,
            initial_status: status::READY.into(),
            ..Default::default()
        };
        match dispatch(store, vault_root, &mut t, opts).await {
            Ok(out) => report.dispatched.push(out),
            Err(e) => report.failures.push((rel_path, e.to_string())),
        }
    }
    Ok(report)
}

/// `[[Architect]]` → `"architect"`. Anything else is normalized
/// to lowercase + dashes. Empty input → `"default"`.
fn project_link_to_slug(link: &str) -> String {
    let trimmed = link.trim_matches(|c: char| c == '[' || c == ']' || c.is_whitespace());
    if trimmed.is_empty() {
        return "default".into();
    }
    trimmed
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

fn is_due(scheduled: Option<&str>, today: NaiveDate) -> bool {
    let Some(s) = scheduled else {
        // No scheduled date — treat as eligible.
        return true;
    };
    let head = s.get(..10).unwrap_or(s);
    NaiveDate::parse_from_str(head, "%Y-%m-%d")
        .map(|d| d <= today)
        .unwrap_or(true)
}

// ────────────────────────────────────────────────────────────────────
// Weekly digest
// ────────────────────────────────────────────────────────────────────

/// One week's worth of agent activity, ready to inline into a
/// project note.
#[derive(Debug, Clone)]
pub struct WeeklyDigest {
    pub week_of: NaiveDate,
    /// All agent tasks that finished (`done` or `archived`) in
    /// this week, sorted by completion timestamp.
    pub completed: Vec<AgentTask>,
}

impl WeeklyDigest {
    /// Render the digest as a markdown section.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        if self.completed.is_empty() {
            return format!(
                "## Agent activity — week of {}\n\n_(no agent tasks completed this week)_\n",
                self.week_of
            );
        }
        let mut out = format!("## Agent activity — week of {}\n\n", self.week_of);
        // Group by project, fall back to `(unscoped)`.
        let mut by_project: std::collections::BTreeMap<String, Vec<&AgentTask>> =
            std::collections::BTreeMap::new();
        for t in &self.completed {
            let key = if t.project.is_empty() {
                "(unscoped)".to_string()
            } else {
                t.project.clone()
            };
            by_project.entry(key).or_default().push(t);
        }
        for (project, tasks) in by_project {
            out.push_str(&format!("### {project}\n\n"));
            for t in tasks {
                let when = t.updated_at.format("%Y-%m-%d %H:%M UTC");
                out.push_str(&format!(
                    "- **{title}** — `{status}` at {when} ({queue})\n",
                    title = t.title,
                    status = t.status,
                    queue = t.queue_id,
                ));
            }
            out.push('\n');
        }
        out
    }
}

/// Read the agent tasks `queue_id` that landed in `done` or
/// `archived` between Monday and Sunday of `week_of`'s week.
/// Pass any date in the week — we normalize to Monday.
pub async fn weekly_digest(
    store: &agent_tasks::Store,
    queue_id: &str,
    week_of: NaiveDate,
) -> Result<WeeklyDigest, DispatchError> {
    let monday = monday_of_week(week_of);
    let sunday_end = monday + Duration::days(7);
    let rows = AgentTaskEntity::find()
        .filter(AgentTaskColumn::QueueId.eq(queue_id))
        .filter(AgentTaskColumn::Status.is_in([status::DONE, status::ARCHIVED]))
        .filter(AgentTaskColumn::UpdatedAt.gte(monday.and_hms_opt(0, 0, 0).unwrap().and_utc()))
        .filter(AgentTaskColumn::UpdatedAt.lt(sunday_end.and_hms_opt(0, 0, 0).unwrap().and_utc()))
        .all(store.conn())
        .await
        .map_err(|e| DispatchError::Agent(agent_proto::AgentError::Backend(e.to_string())))?;
    let mut completed: Vec<AgentTask> = rows.into_iter().map(model_to_agent_task).collect();
    completed.sort_by_key(|t| t.updated_at);
    Ok(WeeklyDigest {
        week_of: monday,
        completed,
    })
}

/// Convenience for the "since this morning" variant the
/// project pages might prefer over a weekly cadence.
pub async fn digest_since(
    store: &agent_tasks::Store,
    queue_id: &str,
    since: chrono::DateTime<Utc>,
) -> Result<Vec<AgentTask>, DispatchError> {
    let rows = AgentTaskEntity::find()
        .filter(AgentTaskColumn::QueueId.eq(queue_id))
        .filter(AgentTaskColumn::Status.is_in([status::DONE, status::ARCHIVED]))
        .filter(AgentTaskColumn::UpdatedAt.gte(since))
        .all(store.conn())
        .await
        .map_err(|e| DispatchError::Agent(agent_proto::AgentError::Backend(e.to_string())))?;
    let mut completed: Vec<AgentTask> = rows.into_iter().map(model_to_agent_task).collect();
    completed.sort_by_key(|t| t.updated_at);
    Ok(completed)
}

fn monday_of_week(d: NaiveDate) -> NaiveDate {
    let dow = d.weekday().num_days_from_monday() as i64;
    d - Duration::days(dow)
}

fn model_to_agent_task(m: agent_tasks::entity::AgentTaskModel) -> AgentTask {
    AgentTask {
        id: m.id,
        queue_id: m.queue_id,
        title: m.title,
        description: m.description,
        status: m.status,
        priority: m.priority,
        labels: m.labels,
        source_task: m.source_task,
        project: m.project,
        agent_profile: m.agent_profile,
        claimed_by: m.claimed_by,
        claim_expires_at: m.claim_expires_at,
        worker_pid: m.worker_pid,
        result_blob: m.result_blob,
        linked_session_id: m.linked_session_id,
        created_at: m.created_at,
        updated_at: m.updated_at,
        archived_at: m.archived_at,
    }
}
