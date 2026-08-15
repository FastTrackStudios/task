//! Persistence for [`Run`] — every attempt, kept.
//!
//! Attempts are append-only in spirit: a retry creates a new row
//! rather than overwriting the last one, because the history *is*
//! the diagnostic.

use std::time::Duration;

use agent_proto::error::AgentError;
use agent_proto::run::{FinishRun, Run, RunFilter, RunStatus, StartRun};
use agent_proto::run_event::{RunEvent, RunEventEnvelope, RunSnapshot, append_tail};
use agent_proto::service::run_stream::{RunStream, RunStreamStreamSource};
use agent_proto::service::runs::Runs;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, Value};
use uuid::Uuid;

/// How long an in-progress run may go without a heartbeat before a
/// sweep calls it stale. Matches the runner window — one machine,
/// one notion of "recently".
pub const RUN_STALE_AFTER: Duration = Duration::from_secs(120);

/// Live, in-memory state for a run: what it is doing and the tail
/// of its output. Deliberately not persisted — output is ephemeral,
/// and a restart losing a tail costs nothing a viewer cares about.
#[derive(Debug, Default, Clone)]
struct Live {
    activity: String,
    tail: String,
}

#[derive(Clone)]
pub struct RunStore {
    conn: DatabaseConnection,
    events: architect::PubSub<RunEventEnvelope>,
    live: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<Uuid, Live>>>,
}

fn ts(v: DateTime<Utc>) -> String {
    v.to_rfc3339()
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

impl std::fmt::Debug for RunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunStore").finish_non_exhaustive()
    }
}

impl RunStore {
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self {
            conn,
            events: architect::PubSub::sliding(256),
            live: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// The hub every run event is published into.
    #[must_use]
    pub fn events(&self) -> &architect::PubSub<RunEventEnvelope> {
        &self.events
    }

    /// Fold an event into the live state and broadcast it.
    fn emit(&self, run: Uuid, ticket: Uuid, event: RunEvent) {
        if let Ok(mut map) = self.live.lock() {
            let entry = map.entry(run).or_default();
            match &event {
                RunEvent::Output(chunk) => append_tail(&mut entry.tail, chunk),
                RunEvent::Activity(what) => entry.activity = what.clone(),
                _ => {}
            }
        }
        self.events.publish(RunEventEnvelope {
            run,
            ticket,
            event,
            at: Utc::now(),
        });
    }

    fn backend(&self) -> sea_orm::DatabaseBackend {
        self.conn.get_database_backend()
    }

    async fn exec(&self, sql: &str, values: Vec<Value>) -> Result<(), AgentError> {
        self.conn
            .execute(Statement::from_sql_and_values(self.backend(), sql, values))
            .await
            .map_err(|e| AgentError::Backend(format!("runs: {e}")))?;
        Ok(())
    }

    async fn rows(&self, sql: &str, values: Vec<Value>) -> Result<Vec<Run>, AgentError> {
        let rows = self
            .conn
            .query_all(Statement::from_sql_and_values(self.backend(), sql, values))
            .await
            .map_err(|e| AgentError::Backend(format!("runs: {e}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let get = |c: &str| -> Result<String, AgentError> {
                r.try_get::<String>("", c)
                    .map_err(|e| AgentError::Backend(format!("runs column {c}: {e}")))
            };
            let id = Uuid::parse_str(&get("id")?)
                .map_err(|e| AgentError::Backend(format!("run id: {e}")))?;
            let ticket = Uuid::parse_str(&get("ticket")?)
                .map_err(|e| AgentError::Backend(format!("run ticket: {e}")))?;
            let parent = {
                let p = get("parent")?;
                if p.is_empty() {
                    None
                } else {
                    Uuid::parse_str(&p).ok()
                }
            };
            let exit = get("exit_code")?;
            out.push(Run {
                id,
                ticket,
                runner: get("runner")?,
                parent,
                branch: get("branch")?,
                worktree_path: get("worktree_path")?,
                session_path: get("session_path")?,
                status: RunStatus::parse(&get("status")?).unwrap_or(RunStatus::Dead),
                exit_code: if exit.is_empty() {
                    None
                } else {
                    exit.parse().ok()
                },
                started_at: parse_ts(&get("started_at")?).unwrap_or_else(Utc::now),
                heartbeat_at: parse_ts(&get("heartbeat_at")?),
                finished_at: parse_ts(&get("finished_at")?),
            });
        }
        Ok(out)
    }

    async fn one(&self, id: Uuid) -> Result<Run, AgentError> {
        self.rows(
            "SELECT * FROM agent_runs WHERE id = ?",
            vec![id.to_string().into()],
        )
        .await?
        .pop()
        .ok_or_else(|| AgentError::AgentTaskNotFound(id.to_string()))
    }

    /// Attempts for one ticket, newest first.
    ///
    /// # Errors
    ///
    /// [`AgentError`] on a storage failure.
    pub async fn for_ticket(&self, ticket: Uuid) -> Result<Vec<Run>, AgentError> {
        self.rows(
            "SELECT * FROM agent_runs WHERE ticket = ? ORDER BY started_at DESC",
            vec![ticket.to_string().into()],
        )
        .await
    }
}

impl Runs for RunStore {
    async fn start_run(&self, start: StartRun) -> Result<Run, AgentError> {
        let now = Utc::now();
        let id = Uuid::new_v4();
        self.exec(
            "INSERT INTO agent_runs (id, ticket, runner, parent, branch, worktree_path, \
             session_path, status, exit_code, started_at, heartbeat_at, finished_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            vec![
                id.to_string().into(),
                start.ticket.to_string().into(),
                start.runner.into(),
                start.parent.map(|p| p.to_string()).unwrap_or_default().into(),
                start.branch.into(),
                start.worktree_path.into(),
                start.session_path.into(),
                RunStatus::InProgress.as_str().into(),
                String::new().into(),
                ts(now).into(),
                ts(now).into(),
                String::new().into(),
            ],
        )
        .await?;
        let created = self.one(id).await?;
        self.emit(created.id, created.ticket, RunEvent::Status(RunStatus::InProgress));
        created_ok(created)
    }

    async fn beat_run(&self, run_id: Uuid) -> Result<(), AgentError> {
        self.exec(
            "UPDATE agent_runs SET heartbeat_at = ?, status = CASE WHEN status = 'stale' \
             THEN 'in-progress' ELSE status END WHERE id = ?",
            vec![ts(Utc::now()).into(), run_id.to_string().into()],
        )
        .await
    }

    async fn finish_run(&self, finish: FinishRun) -> Result<Run, AgentError> {
        // A worktree still on disk is exactly what needs-cleanup
        // means, so the verdict and the disk state are recorded
        // together rather than in two writes that could disagree.
        let status = if finish.worktree_kept {
            RunStatus::NeedsCleanup
        } else if finish.passed {
            RunStatus::Passed
        } else {
            RunStatus::Failed
        };
        self.exec(
            "UPDATE agent_runs SET status = ?, exit_code = ?, finished_at = ? WHERE id = ?",
            vec![
                status.as_str().into(),
                finish
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_default()
                    .into(),
                ts(Utc::now()).into(),
                finish.run.to_string().into(),
            ],
        )
        .await?;
        let done = self.one(finish.run).await?;
        self.emit(
            done.id,
            done.ticket,
            RunEvent::Verdict {
                passed: finish.passed,
                exit_code: finish.exit_code,
            },
        );
        self.emit(done.id, done.ticket, RunEvent::Status(done.status));
        Ok(done)
    }

    async fn get_run(&self, run_id: Uuid) -> Result<Run, AgentError> {
        self.one(run_id).await
    }

    async fn list_runs(&self, filter: RunFilter) -> Result<Vec<Run>, AgentError> {
        let mut sql = String::from("SELECT * FROM agent_runs WHERE 1=1");
        let mut vals: Vec<Value> = Vec::new();
        if let Some(t) = filter.ticket {
            sql.push_str(" AND ticket = ?");
            vals.push(t.to_string().into());
        }
        if !filter.runner.is_empty() {
            sql.push_str(" AND runner = ?");
            vals.push(filter.runner.into());
        }
        if let Some(p) = filter.parent {
            sql.push_str(" AND parent = ?");
            vals.push(p.to_string().into());
        }
        if let Some(s) = filter.status {
            sql.push_str(" AND status = ?");
            vals.push(s.as_str().into());
        }
        sql.push_str(" ORDER BY started_at DESC");
        if filter.limit > 0 {
            sql.push_str(&format!(" LIMIT {}", filter.limit));
        }
        self.rows(&sql, vals).await
    }

    async fn archive_run(&self, run_id: Uuid) -> Result<Run, AgentError> {
        self.exec(
            "UPDATE agent_runs SET status = ? WHERE id = ?",
            vec![
                RunStatus::Archived.as_str().into(),
                run_id.to_string().into(),
            ],
        )
        .await?;
        self.one(run_id).await
    }

    async fn sweep_stale_runs(&self) -> Result<u32, AgentError> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(RUN_STALE_AFTER)
                .map_err(|e| AgentError::Backend(format!("stale window: {e}")))?;
        let before = self
            .list_runs(RunFilter {
                status: Some(RunStatus::InProgress),
                ..Default::default()
            })
            .await?;
        let lapsed: Vec<&Run> = before
            .iter()
            .filter(|r| r.heartbeat_at.is_none_or(|h| h < cutoff))
            .collect();
        for r in &lapsed {
            self.exec(
                "UPDATE agent_runs SET status = ? WHERE id = ?",
                vec![RunStatus::Stale.as_str().into(), r.id.to_string().into()],
            )
            .await?;
        }
        Ok(u32::try_from(lapsed.len()).unwrap_or(u32::MAX))
    }
}

/// The `#[subscribe]` contract: one hub, every run.
impl RunStreamStreamSource for RunStore {
    fn run_events_hub(&self) -> &architect::PubSub<RunEventEnvelope> {
        &self.events
    }
}

impl RunStream for RunStore {
    async fn snapshot(&self, run: Uuid) -> Result<RunSnapshot, AgentError> {
        let row = self.one(run).await?;
        let live = self
            .live
            .lock()
            .ok()
            .and_then(|m| m.get(&run).cloned())
            .unwrap_or_default();
        Ok(RunSnapshot {
            run: row.id,
            ticket: row.ticket,
            status: row.status,
            activity: live.activity,
            tail: live.tail,
        })
    }

    async fn publish(&self, run: Uuid, event: RunEvent) -> Result<(), AgentError> {
        // Resolve the ticket so every envelope is self-describing —
        // a fleet view should not have to join to know what it is
        // looking at.
        let ticket = self.one(run).await?.ticket;
        self.emit(run, ticket, event);
        Ok(())
    }
}

/// Tiny helper so `start_run` can emit before returning without an
/// extra clone dance at the call site.
fn created_ok(run: Run) -> Result<Run, AgentError> {
    Ok(run)
}
