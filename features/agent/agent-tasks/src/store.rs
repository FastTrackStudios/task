//! SeaORM-backed implementation of `agent_proto::service::tasks::AgentTaskQueue`.
//!
//! One `Store` = one connection. The dispatcher runs server-side
//! and owns the connection pool; clients reach it through the
//! vox RPC plumbing emitted by `#[architect::rpc]` on the
//! `AgentTaskQueue` trait.
//!
//! ## What this crate adds over the architect-emitted CRUD
//!
//! `architect::Entity` already emits per-table `AgentTaskRepo`,
//! `QueueRepo`, etc. with `list / get / create / update / delete`.
//! That's enough for *most* writes. The four ops that need their
//! own home here are:
//!
//! - [`Store::claim_agent_task`] — atomic `UPDATE … WHERE
//!   status = 'ready' AND claimed_by = ''` so two workers can't
//!   both grab the same row.
//! - [`Store::set_agent_task_status`] — rejects `"running"`
//!   (must claim) and `Done` (must complete with a result).
//! - [`Store::complete_agent_task`] — checks that no incoming
//!   `blocks` link still points at a non-`done` task before
//!   flipping the row to `done`.
//! - [`Store::read_queue`] — one-shot snapshot + event-log
//!   watermark so a subscribing client can resume the tail
//!   without a gap.

use agent_proto::AgentError;
use agent_proto::service::tasks::AgentTaskQueue;
use agent_proto::tasks::{AgentTask, AgentTaskLink, Queue, QueueFilter, QueueSnapshot, status};
use chrono::Utc;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use crate::entity::{
    AgentTaskActive, AgentTaskColumn, AgentTaskEntity, AgentTaskLinkColumn, AgentTaskLinkEntity,
    AgentTaskLinkModel, AgentTaskModel, QueueActive, QueueColumn, QueueEntity, QueueModel,
};
use crate::error::TasksDbError;

const QUEUE_EVENTS_TABLE: &str = "queue_events";

#[derive(Clone)]
pub struct Store {
    conn: DatabaseConnection,
}

impl Store {
    /// Wrap an open SeaORM connection. The caller is responsible
    /// for running [`crate::Migrator`] against this connection
    /// before instantiating a `Store`.
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    /// Borrow the underlying connection. Useful when a caller
    /// wants to drive the architect-emitted `<Name>RepoStorage`
    /// types directly against the same DB.
    #[must_use]
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    // ── Queue (slim helpers — full CRUD is via QueueRepo) ──────────

    pub async fn upsert_queue_row(&self, queue: Queue) -> Result<Queue, TasksDbError> {
        let now = Utc::now();
        let active = QueueActive {
            id: Set(queue.id.clone()),
            label: Set(queue.label),
            created_at: Set(if queue.created_at.timestamp() == 0 {
                now
            } else {
                queue.created_at
            }),
            updated_at: Set(now),
        };
        QueueEntity::insert(active)
            .on_conflict(
                OnConflict::column(QueueColumn::Id)
                    .update_columns([QueueColumn::Label, QueueColumn::UpdatedAt])
                    .to_owned(),
            )
            .exec(&self.conn)
            .await?;
        self.read_queue_row(&queue.id).await
    }

    async fn read_queue_row(&self, queue_id: &str) -> Result<Queue, TasksDbError> {
        QueueEntity::find_by_id(queue_id.to_string())
            .one(&self.conn)
            .await?
            .map(model_to_queue)
            .ok_or_else(|| TasksDbError::NotFound {
                kind: "queue",
                id: queue_id.to_string(),
            })
    }

    // ── Agent tasks ─────────────────────────────────────────────────

    pub async fn insert_agent_task(&self, mut task: AgentTask) -> Result<AgentTask, TasksDbError> {
        if task.status == status::RUNNING {
            return Err(TasksDbError::InvalidTransition(
                "cannot insert with status=running; use claim_agent_task".into(),
            ));
        }
        if !status::is_known(&task.status) {
            return Err(TasksDbError::Invalid(format!(
                "unknown status {:?}",
                task.status
            )));
        }
        if task.id.is_nil() {
            task.id = Uuid::new_v4();
        }
        let now = Utc::now();
        if task.created_at.timestamp() == 0 {
            task.created_at = now;
        }
        task.updated_at = now;

        agent_task_to_active(&task).insert(&self.conn).await?;
        log_event(&self.conn, &task.queue_id, Some(task.id), "created", "{}").await?;
        self.fetch_task(task.id).await
    }

    async fn fetch_task(&self, id: Uuid) -> Result<AgentTask, TasksDbError> {
        AgentTaskEntity::find_by_id(id)
            .one(&self.conn)
            .await?
            .map(model_to_agent_task)
            .ok_or_else(|| TasksDbError::NotFound {
                kind: "agent_task",
                id: id.to_string(),
            })
    }

    async fn check_unblocked(&self, id: Uuid) -> Result<(), TasksDbError> {
        let blocking = AgentTaskLinkEntity::find()
            .filter(AgentTaskLinkColumn::ToTask.eq(id))
            .filter(AgentTaskLinkColumn::Kind.eq(agent_proto::tasks::link_kind::BLOCKS))
            .all(&self.conn)
            .await?;
        if blocking.is_empty() {
            return Ok(());
        }
        let from_ids: Vec<Uuid> = blocking.iter().map(|l| l.from_task).collect();
        let still_open = AgentTaskEntity::find()
            .filter(AgentTaskColumn::Id.is_in(from_ids))
            .filter(AgentTaskColumn::Status.ne(status::DONE))
            .count(&self.conn)
            .await?;
        if still_open > 0 {
            return Err(TasksDbError::InvalidTransition(format!(
                "agent task {id} has {still_open} unresolved `blocks` dependencies"
            )));
        }
        Ok(())
    }

    async fn list_tasks_filtered(
        &self,
        queue_id: &str,
        filter: &QueueFilter,
    ) -> Result<Vec<AgentTask>, TasksDbError> {
        let mut q = AgentTaskEntity::find().filter(AgentTaskColumn::QueueId.eq(queue_id));
        if !filter.include_archived {
            q = q.filter(AgentTaskColumn::Status.ne(status::ARCHIVED));
        }
        if !filter.agent_profile.is_empty() {
            q = q.filter(AgentTaskColumn::AgentProfile.eq(filter.agent_profile.clone()));
        }
        if !filter.assignee.is_empty() {
            q = q.filter(AgentTaskColumn::ClaimedBy.eq(filter.assignee.clone()));
        }
        if !filter.only_handle.is_empty() {
            q = q.filter(AgentTaskColumn::ClaimedBy.eq(filter.only_handle.clone()));
        }
        if !filter.linked_session_id.is_empty() {
            q = q.filter(AgentTaskColumn::LinkedSessionId.eq(filter.linked_session_id.clone()));
        }
        let rows = q
            .order_by_asc(AgentTaskColumn::Priority)
            .order_by_asc(AgentTaskColumn::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(rows.into_iter().map(model_to_agent_task).collect())
    }

    async fn latest_event_id(&self, queue_id: &str) -> Result<u64, TasksDbError> {
        // No SeaORM entity for queue_events (it's an internal append-only
        // table). Drop to a raw query.
        use sea_orm::Statement;
        let backend = self.conn.get_database_backend();
        let stmt = Statement::from_sql_and_values(
            backend,
            format!("SELECT COALESCE(MAX(id), 0) FROM {QUEUE_EVENTS_TABLE} WHERE queue_id = ?"),
            vec![queue_id.into()],
        );
        let row = self
            .conn
            .query_one(stmt)
            .await?
            .ok_or_else(|| TasksDbError::Db(DbErr::RecordNotFound("queue_events".into())))?;
        let v: i64 = row.try_get_by_index(0)?;
        Ok(v.max(0) as u64)
    }
}

impl AgentTaskQueue for Store {
    async fn read_queue(
        &self,
        queue_id: String,
        filter: QueueFilter,
    ) -> Result<QueueSnapshot, AgentError> {
        let queue = self
            .read_queue_row(&queue_id)
            .await
            .map_err(AgentError::from)?;
        let tasks = self
            .list_tasks_filtered(&queue_id, &filter)
            .await
            .map_err(AgentError::from)?;
        let latest_event_id = self
            .latest_event_id(&queue_id)
            .await
            .map_err(AgentError::from)?;
        Ok(QueueSnapshot {
            queue,
            tasks,
            latest_event_id,
            read_only: false,
        })
    }

    async fn claim_agent_task(
        &self,
        agent_task_id: String,
        handle: String,
    ) -> Result<AgentTask, AgentError> {
        if handle.is_empty() {
            return Err(TasksDbError::Invalid("handle must be non-empty".into()).into());
        }
        let id = parse_uuid(&agent_task_id).map_err(AgentError::from)?;
        let now = Utc::now();
        // Atomic flip: status=ready AND claimed_by='' → running, claimed_by=handle.
        let updated = AgentTaskEntity::update_many()
            .col_expr(AgentTaskColumn::Status, Expr::value(status::RUNNING))
            .col_expr(AgentTaskColumn::ClaimedBy, Expr::value(handle.clone()))
            .col_expr(AgentTaskColumn::UpdatedAt, Expr::value(now))
            .filter(AgentTaskColumn::Id.eq(id))
            .filter(AgentTaskColumn::Status.eq(status::READY))
            .filter(AgentTaskColumn::ClaimedBy.eq(""))
            .exec(&self.conn)
            .await
            .map_err(|e| AgentError::from(TasksDbError::from(e)))?;
        if updated.rows_affected == 0 {
            // Disambiguate.
            let current = self.fetch_task(id).await.map_err(AgentError::from)?;
            return Err(TasksDbError::Conflict(format!(
                "agent task {id} is {} (claimed_by={:?})",
                current.status, current.claimed_by
            ))
            .into());
        }
        let qid = queue_id_of(&self.conn, id).await?;
        log_event(
            &self.conn,
            &qid,
            Some(id),
            "claim",
            &serde_json::json!({ "by": handle }).to_string(),
        )
        .await
        .map_err(AgentError::from)?;
        self.fetch_task(id).await.map_err(AgentError::from)
    }

    async fn set_agent_task_status(
        &self,
        agent_task_id: String,
        new_status: String,
    ) -> Result<AgentTask, AgentError> {
        let id = parse_uuid(&agent_task_id).map_err(AgentError::from)?;
        if new_status == status::RUNNING {
            return Err(TasksDbError::InvalidTransition(
                "use claim_agent_task to transition into running".into(),
            )
            .into());
        }
        if !status::is_known(&new_status) {
            return Err(TasksDbError::Invalid(format!("unknown status {new_status:?}")).into());
        }
        if new_status == status::DONE {
            self.check_unblocked(id).await.map_err(AgentError::from)?;
        }
        let now = Utc::now();
        let is_done = new_status == status::DONE;
        let is_archived = new_status == status::ARCHIVED;
        let mut q = AgentTaskEntity::update_many()
            .col_expr(AgentTaskColumn::Status, Expr::value(new_status.clone()))
            .col_expr(AgentTaskColumn::UpdatedAt, Expr::value(now));
        if is_done || is_archived {
            q = q
                .col_expr(AgentTaskColumn::ClaimedBy, Expr::value(""))
                .col_expr(
                    AgentTaskColumn::ClaimExpiresAt,
                    Expr::value(None::<chrono::DateTime<Utc>>),
                );
        }
        if is_archived {
            q = q.col_expr(AgentTaskColumn::ArchivedAt, Expr::value(now));
        }
        let updated = q
            .filter(AgentTaskColumn::Id.eq(id))
            .exec(&self.conn)
            .await
            .map_err(|e| AgentError::from(TasksDbError::from(e)))?;
        if updated.rows_affected == 0 {
            return Err(TasksDbError::NotFound {
                kind: "agent_task",
                id: agent_task_id,
            }
            .into());
        }
        let qid = queue_id_of(&self.conn, id).await?;
        log_event(
            &self.conn,
            &qid,
            Some(id),
            "status",
            &serde_json::json!({ "to": new_status }).to_string(),
        )
        .await
        .map_err(AgentError::from)?;
        self.fetch_task(id).await.map_err(AgentError::from)
    }

    async fn complete_agent_task(
        &self,
        agent_task_id: String,
        result_blob: String,
    ) -> Result<AgentTask, AgentError> {
        let id = parse_uuid(&agent_task_id).map_err(AgentError::from)?;
        self.check_unblocked(id).await.map_err(AgentError::from)?;
        let now = Utc::now();
        let updated = AgentTaskEntity::update_many()
            .col_expr(AgentTaskColumn::Status, Expr::value(status::DONE))
            .col_expr(AgentTaskColumn::ResultBlob, Expr::value(result_blob))
            .col_expr(AgentTaskColumn::ClaimedBy, Expr::value(""))
            .col_expr(
                AgentTaskColumn::ClaimExpiresAt,
                Expr::value(None::<chrono::DateTime<Utc>>),
            )
            .col_expr(AgentTaskColumn::UpdatedAt, Expr::value(now))
            .filter(AgentTaskColumn::Id.eq(id))
            .exec(&self.conn)
            .await
            .map_err(|e| AgentError::from(TasksDbError::from(e)))?;
        if updated.rows_affected == 0 {
            return Err(TasksDbError::NotFound {
                kind: "agent_task",
                id: agent_task_id,
            }
            .into());
        }
        let qid = queue_id_of(&self.conn, id).await?;
        log_event(&self.conn, &qid, Some(id), "done", "{}")
            .await
            .map_err(AgentError::from)?;
        self.fetch_task(id).await.map_err(AgentError::from)
    }

    async fn link_agent_task_to_session(
        &self,
        agent_task_id: String,
        session_id: String,
    ) -> Result<AgentTask, AgentError> {
        let id = parse_uuid(&agent_task_id).map_err(AgentError::from)?;
        let now = Utc::now();
        let updated = AgentTaskEntity::update_many()
            .col_expr(AgentTaskColumn::LinkedSessionId, Expr::value(session_id))
            .col_expr(AgentTaskColumn::UpdatedAt, Expr::value(now))
            .filter(AgentTaskColumn::Id.eq(id))
            .exec(&self.conn)
            .await
            .map_err(|e| AgentError::from(TasksDbError::from(e)))?;
        if updated.rows_affected == 0 {
            return Err(TasksDbError::NotFound {
                kind: "agent_task",
                id: agent_task_id,
            }
            .into());
        }
        self.fetch_task(id).await.map_err(AgentError::from)
    }

    async fn list_agent_task_links(
        &self,
        queue_id: String,
    ) -> Result<Vec<AgentTaskLink>, AgentError> {
        // Find every link whose `from_task` belongs to this queue.
        // (Two-task links are symmetric for this purpose; one side
        // belonging to the queue is enough to surface the edge.)
        let task_ids: Vec<Uuid> = AgentTaskEntity::find()
            .filter(AgentTaskColumn::QueueId.eq(queue_id.clone()))
            .all(&self.conn)
            .await
            .map_err(|e| AgentError::from(TasksDbError::from(e)))?
            .into_iter()
            .map(|t| t.id)
            .collect();
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }
        let links = AgentTaskLinkEntity::find()
            .filter(AgentTaskLinkColumn::FromTask.is_in(task_ids.clone()))
            .all(&self.conn)
            .await
            .map_err(|e| AgentError::from(TasksDbError::from(e)))?;
        Ok(links.into_iter().map(model_to_link).collect())
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn parse_uuid(s: &str) -> Result<Uuid, TasksDbError> {
    s.parse::<Uuid>()
        .map_err(|_| TasksDbError::Invalid(format!("not a valid uuid: {s:?}")))
}

fn model_to_queue(m: QueueModel) -> Queue {
    Queue {
        id: m.id,
        label: m.label,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn model_to_agent_task(m: AgentTaskModel) -> AgentTask {
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

fn agent_task_to_active(t: &AgentTask) -> AgentTaskActive {
    AgentTaskActive {
        id: Set(t.id),
        queue_id: Set(t.queue_id.clone()),
        title: Set(t.title.clone()),
        description: Set(t.description.clone()),
        status: Set(t.status.clone()),
        priority: Set(t.priority),
        labels: Set(t.labels.clone()),
        source_task: Set(t.source_task.clone()),
        project: Set(t.project.clone()),
        agent_profile: Set(t.agent_profile.clone()),
        claimed_by: Set(t.claimed_by.clone()),
        claim_expires_at: Set(t.claim_expires_at),
        worker_pid: Set(t.worker_pid),
        result_blob: Set(t.result_blob.clone()),
        linked_session_id: Set(t.linked_session_id.clone()),
        created_at: Set(t.created_at),
        updated_at: Set(t.updated_at),
        archived_at: Set(t.archived_at),
    }
}

fn model_to_link(m: AgentTaskLinkModel) -> AgentTaskLink {
    AgentTaskLink {
        id: m.id,
        from_task: m.from_task,
        to_task: m.to_task,
        kind: m.kind,
        created_at: m.created_at,
    }
}

async fn queue_id_of<C: ConnectionTrait>(conn: &C, id: Uuid) -> Result<String, AgentError> {
    let row = AgentTaskEntity::find_by_id(id)
        .one(conn)
        .await
        .map_err(|e| AgentError::from(TasksDbError::from(e)))?
        .ok_or_else(|| {
            AgentError::from(TasksDbError::NotFound {
                kind: "agent_task",
                id: id.to_string(),
            })
        })?;
    Ok(row.queue_id)
}

async fn log_event<C: ConnectionTrait>(
    conn: &C,
    queue_id: &str,
    agent_task_id: Option<Uuid>,
    kind: &str,
    payload_json: &str,
) -> Result<(), TasksDbError> {
    use sea_orm::Statement;
    let backend = conn.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        format!(
            "INSERT INTO {QUEUE_EVENTS_TABLE} (queue_id, agent_task_id, kind, payload, ts) \
             VALUES (?, ?, ?, ?, ?)"
        ),
        vec![
            queue_id.into(),
            agent_task_id.into(),
            kind.into(),
            payload_json.into(),
            Utc::now().timestamp_millis().into(),
        ],
    );
    conn.execute(stmt).await?;
    Ok(())
}
