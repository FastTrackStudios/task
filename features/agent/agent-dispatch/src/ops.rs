//! Dispatch + completion operations.

use std::path::Path;

use agent_proto::service::tasks::AgentTaskQueue;
use agent_proto::tasks::{AgentTask, Labels, status};
use agent_tasks::entity::{AgentTaskActive, AgentTaskEntity};
use chrono::Utc;
use sea_orm::{EntityTrait, Set};
use task::{TaskInfo, parse_str, serialize_task};
use uuid::Uuid;

use crate::error::DispatchError;

/// Options for [`dispatch`]. The caller can override per-card
/// values; defaults pull from the source task where possible.
#[derive(Debug, Clone)]
pub struct DispatchOptions {
    /// Queue to drop the agent task into. Must already exist —
    /// queue creation is a deliberate user action, not a side
    /// effect of dispatch.
    pub queue_id: String,
    /// Initial status. Defaults to `triage` so the user reviews
    /// before a worker picks it up. Pass `status::READY` to
    /// skip triage.
    pub initial_status: String,
    /// Override the agent profile from the task note. Empty =
    /// inherit `TaskInfo::agent_profile`. If both are empty,
    /// any worker can claim.
    pub agent_profile_override: String,
    /// 0..=4, P0 (urgent) to P4 (later). Defaults to the
    /// task note's priority mapped: `urgent=0, high=1,
    /// normal=2, low=3`, anything else → 2.
    pub priority_override: Option<i32>,
    /// Optional explicit labels. Empty = use the task note's
    /// `tags` minus the literal `task` tag.
    pub labels_override: Vec<String>,
}

impl Default for DispatchOptions {
    fn default() -> Self {
        Self {
            queue_id: "default".into(),
            initial_status: status::TRIAGE.into(),
            agent_profile_override: String::new(),
            priority_override: None,
            labels_override: Vec::new(),
        }
    }
}

/// Result of a successful `dispatch` — the freshly inserted
/// agent task + the on-disk path of the patched task note.
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    pub agent_task: AgentTask,
    pub task_path: std::path::PathBuf,
}

/// Dispatch a task note to an agent queue. Writes both sides
/// (DB row + frontmatter `dispatchedAgentTasks` entry) so the
/// link is bidirectional.
///
/// Failure modes:
/// - [`DispatchError::Agent`] if the queue insert fails.
/// - [`DispatchError::TaskWrite`] if the frontmatter patch
///   fails *after* the DB insert succeeded — in that case the
///   DB row exists but the task note isn't updated. The reconcile
///   pass (slice 5+) will catch + auto-link these.
pub async fn dispatch(
    store: &agent_tasks::Store,
    vault_root: &Path,
    task: &mut TaskInfo,
    options: DispatchOptions,
) -> Result<DispatchOutcome, DispatchError> {
    let agent_task = insert_agent_task(store, task, &options).await?;
    task.dispatched_agent_tasks.push(agent_task.id.to_string());
    let task_path = task::write_task(vault_root, task, true)?;
    Ok(DispatchOutcome {
        agent_task,
        task_path,
    })
}

async fn insert_agent_task(
    store: &agent_tasks::Store,
    task: &TaskInfo,
    options: &DispatchOptions,
) -> Result<AgentTask, DispatchError> {
    let now = Utc::now();
    let id = Uuid::new_v4();
    let priority = options
        .priority_override
        .unwrap_or_else(|| map_priority(&task.priority));
    let labels = if options.labels_override.is_empty() {
        task.tags
            .iter()
            .filter(|t| !t.eq_ignore_ascii_case("task"))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        options.labels_override.clone()
    };
    let agent_profile = if options.agent_profile_override.is_empty() {
        task.agent_profile.clone()
    } else {
        options.agent_profile_override.clone()
    };
    let project = task.projects.first().cloned().unwrap_or_default();

    let active = AgentTaskActive {
        id: Set(id),
        queue_id: Set(options.queue_id.clone()),
        title: Set(task.title.clone()),
        description: Set(task.details.clone()),
        status: Set(options.initial_status.clone()),
        priority: Set(priority),
        labels: Set(Labels(labels)),
        source_task: Set(task.path.clone()),
        project: Set(project),
        agent_profile: Set(agent_profile),
        claimed_by: Set(String::new()),
        claim_expires_at: Set(None),
        worker_pid: Set(0),
        result_blob: Set(String::new()),
        linked_session_id: Set(String::new()),
        created_at: Set(now),
        updated_at: Set(now),
        archived_at: Set(None),
    };
    AgentTaskEntity::insert(active)
        .exec(store.conn())
        .await
        .map_err(|e| DispatchError::Agent(agent_proto::AgentError::Backend(e.to_string())))?;
    let row = AgentTaskEntity::find_by_id(id)
        .one(store.conn())
        .await
        .map_err(|e| DispatchError::Agent(agent_proto::AgentError::Backend(e.to_string())))?
        .ok_or_else(|| {
            DispatchError::Agent(agent_proto::AgentError::AgentTaskNotFound(id.to_string()))
        })?;
    Ok(AgentTask {
        id: row.id,
        queue_id: row.queue_id,
        title: row.title,
        description: row.description,
        status: row.status,
        priority: row.priority,
        labels: row.labels,
        source_task: row.source_task,
        project: row.project,
        agent_profile: row.agent_profile,
        claimed_by: row.claimed_by,
        claim_expires_at: row.claim_expires_at,
        worker_pid: row.worker_pid,
        result_blob: row.result_blob,
        linked_session_id: row.linked_session_id,
        created_at: row.created_at,
        updated_at: row.updated_at,
        archived_at: row.archived_at,
    })
}

fn map_priority(s: &str) -> i32 {
    match s.to_ascii_lowercase().as_str() {
        "urgent" | "highest" | "p0" => 0,
        "high" | "p1" => 1,
        "normal" | "medium" | "p2" => 2,
        "low" | "p3" => 3,
        "lowest" | "p4" => 4,
        _ => 2,
    }
}

/// Complete a dispatched agent task and fold the result back
/// into the source task note. Two-step:
///
/// 1. `store.complete_agent_task(...)` — agent task → `done`,
///    `result_blob` populated, enforces dependency rule.
/// 2. Re-read the source task note from disk and update:
///    - If `recurrence` is set → append today's date to
///      `complete_instances`. Task status stays `open`; the
///      next instance is the next scheduled occurrence.
///    - Otherwise → status → `done`, `completedDate` =
///      today (UTC).
///
/// Step 2 failure does NOT roll back step 1 — the agent task
/// stays `done` and the task note simply hasn't been touched.
/// The reconcile pass picks this up.
pub async fn complete_dispatched(
    store: &agent_tasks::Store,
    vault_root: &Path,
    agent_task_id: &str,
    result_blob: &str,
) -> Result<DispatchOutcome, DispatchError> {
    let agent_task = store
        .complete_agent_task(agent_task_id.to_string(), result_blob.to_string())
        .await?;
    let source = agent_task.source_task.clone();
    if source.is_empty() {
        // Standalone agent task — nothing to write back.
        return Ok(DispatchOutcome {
            agent_task,
            task_path: std::path::PathBuf::new(),
        });
    }
    let abs = vault_root.join(&source);
    if !abs.exists() {
        return Err(DispatchError::SourceTaskMissing { path: source });
    }
    let raw = std::fs::read_to_string(&abs)?;
    let basename = Path::new(&source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut task =
        parse_str(&source, basename, &raw).map_err(|e| DispatchError::Invalid(e.to_string()))?;
    let today = Utc::now().date_naive();
    if task.recurrence.is_some() {
        let today_s = today.format("%Y-%m-%d").to_string();
        if !task.complete_instances.contains(&today_s) {
            task.complete_instances.push(today_s);
        }
    } else {
        task.status = "done".into();
        task.completed_date = Some(today);
    }
    let serialized = serialize_task(&task)?;
    std::fs::write(&abs, serialized)?;
    Ok(DispatchOutcome {
        agent_task,
        task_path: abs,
    })
}
