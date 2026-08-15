//! `TimerService` impl over SeaORM.
//!
//! The store wraps a connection plus a [`ProjectDefaults`]
//! lookup — that's the cascade level (2) of the rate
//! resolver that lives outside the DB (it's read off
//! disk by `features/project`). For tests there's an
//! in-memory impl; in production the server hands a
//! vault-rooted impl.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use timer_proto::service::{
    LogSessionRequest, RateResolution, RateSource, StartTimerRequest, TimerService,
};
use timer_proto::{
    OrgMemberRate, ProjectMemberRate, Tag, TimerError, WorkSession, WorkSessionFilter,
};
use uuid::Uuid;

use crate::entity::{
    OrgMemberRateColumn, OrgMemberRateEntity, OrgMemberRateModel, ProjectMemberRateActive,
    ProjectMemberRateColumn, ProjectMemberRateEntity, ProjectMemberRateModel, WorkSessionActive,
    WorkSessionColumn, WorkSessionEntity, WorkSessionModel,
};
use crate::error::TimerDbError;
use crate::rate_cascade::{CascadeInputs, ResolvedRate, resolve};

/// Lookup hook for the project markdown's `defaultRateCents`
/// + `billableDefault` + `currency` fields. The store calls
/// it during `start_timer` (to seed `billable`) and
/// `resolve_rate` (for cascade level 2).
///
/// Two impls in the wild:
///
/// - [`VaultProjectDefaults`] — production. Walks
///   `<vault_root>/Projects/` and parses with the `project`
///   crate.
/// - [`StaticProjectDefaults`] — tests. Hand-fed map.
#[async_trait::async_trait]
pub trait ProjectDefaults: Send + Sync {
    /// Returns `(default_rate_cents, currency, billable_default)`
    /// for `project_id`. `None` when the project file isn't
    /// found or has no `id` matching.
    async fn lookup(&self, project_id: Uuid) -> Option<(i64, String, bool)>;
}

/// Tests / fixtures.
#[derive(Clone, Default)]
pub struct StaticProjectDefaults {
    pub map: std::collections::HashMap<Uuid, (i64, String, bool)>,
}

#[async_trait::async_trait]
impl ProjectDefaults for StaticProjectDefaults {
    async fn lookup(&self, project_id: Uuid) -> Option<(i64, String, bool)> {
        self.map.get(&project_id).cloned()
    }
}

/// Production impl. Scans `<vault_root>/Projects/*.md` for a
/// matching `id:` on each call. Cheap for now (sequential
/// readdir + frontmatter parse); a cache lives in
/// `vault-live` once we wire that in.
pub struct VaultProjectDefaults {
    pub vault_root: PathBuf,
}

#[async_trait::async_trait]
impl ProjectDefaults for VaultProjectDefaults {
    async fn lookup(&self, project_id: Uuid) -> Option<(i64, String, bool)> {
        let projects_dir = self.vault_root.join("Projects");
        let entries = std::fs::read_dir(&projects_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).ok()?;
            let rel = path
                .strip_prefix(&self.vault_root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let basename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let Ok(p) = project::parse_str(&rel, basename, &raw) else {
                continue;
            };
            if p.id == project_id {
                return Some((p.default_rate_cents, p.currency, p.billable_default));
            }
        }
        None
    }
}

#[derive(Clone)]
pub struct Store {
    conn: DatabaseConnection,
    project_defaults: Arc<dyn ProjectDefaults>,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful *session* mutation publishes the post-write
    /// row here ([`timer_proto::TimerEvent`]); rate edits don't
    /// stream. Sliding mailbox: a slow subscriber loses its *oldest*
    /// queued events, which is correct for state-shaped payloads.
    /// Clones share the hub (`Arc` inside), so the service mount and
    /// the stream mount can each hold a store clone.
    events: architect::PubSub<timer_proto::TimerEvent>,
}

impl Store {
    /// Construct from an already-opened connection. The caller
    /// must run [`crate::Migrator`] before invoking any method.
    pub fn new(conn: DatabaseConnection, project_defaults: Arc<dyn ProjectDefaults>) -> Self {
        Self {
            conn,
            project_defaults,
            events: architect::PubSub::sliding(256),
        }
    }

    /// Publish a session change to every `events` subscriber. Call
    /// only after the write succeeded — subscribers fold these into
    /// state fetched via `list_sessions()`, so a phantom event would
    /// desync them.
    fn publish(&self, event: timer_proto::TimerEvent) {
        self.events.publish(event);
    }

    #[must_use]
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    /// Upsert an org-level member rate (cascade level 3). Sets the
    /// hourly rate (cents) + currency for `(org_id, user_id)`,
    /// updating the existing row if one exists. New sessions logged
    /// for that member snapshot this rate at close time.
    pub async fn set_org_member_rate(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        hourly_cents: i64,
        currency: &str,
    ) -> Result<(), TimerDbError> {
        let existing = OrgMemberRateEntity::find()
            .filter(OrgMemberRateColumn::OrgId.eq(org_id))
            .filter(OrgMemberRateColumn::UserId.eq(user_id))
            .one(&self.conn)
            .await?;
        if let Some(row) = existing {
            let mut active: crate::entity::OrgMemberRateActive = row.into();
            active.hourly_cents = Set(hourly_cents);
            active.currency = Set(currency.to_string());
            active.updated_at = Set(Utc::now());
            active.update(&self.conn).await?;
        } else {
            OrgMemberRateEntity::insert(crate::entity::OrgMemberRateActive {
                id: Set(Uuid::new_v4()),
                org_id: Set(org_id),
                user_id: Set(user_id),
                hourly_cents: Set(hourly_cents),
                currency: Set(currency.to_string()),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
            })
            .exec(&self.conn)
            .await?;
        }
        Ok(())
    }

    // ── Active timer helpers ────────────────────────────────

    async fn read_active(&self, user_id: Uuid) -> Result<Option<WorkSession>, TimerDbError> {
        let row = WorkSessionEntity::find()
            .filter(WorkSessionColumn::UserId.eq(user_id))
            .filter(WorkSessionColumn::EndTime.is_null())
            .one(&self.conn)
            .await?;
        Ok(row.map(model_to_session))
    }

    async fn cascade_for(
        &self,
        user_id: Uuid,
        org_id: Uuid,
        project_id: Option<Uuid>,
    ) -> Result<ResolvedRate, TimerDbError> {
        let mut inputs = CascadeInputs::default();
        if let Some(pid) = project_id {
            if let Some(rate_row) = ProjectMemberRateEntity::find()
                .filter(ProjectMemberRateColumn::ProjectId.eq(pid))
                .filter(ProjectMemberRateColumn::UserId.eq(user_id))
                .one(&self.conn)
                .await?
            {
                // Project-level rates inherit the project's
                // currency from the markdown defaults; we
                // pull that here so the snapshot is complete.
                let currency = self
                    .project_defaults
                    .lookup(pid)
                    .await
                    .map(|(_, c, _)| c)
                    .unwrap_or_default();
                inputs.project_member = Some((rate_row.hourly_cents, currency));
            }
            if let Some((cents, currency, _)) = self.project_defaults.lookup(pid).await {
                inputs.project_default = Some((cents, currency));
            }
        }
        if let Some(rate_row) = OrgMemberRateEntity::find()
            .filter(OrgMemberRateColumn::OrgId.eq(org_id))
            .filter(OrgMemberRateColumn::UserId.eq(user_id))
            .one(&self.conn)
            .await?
        {
            inputs.org_member = Some((rate_row.hourly_cents, rate_row.currency));
        }
        Ok(resolve(&inputs))
    }

    async fn insert_session(
        &self,
        req: &StartTimerRequest,
        start: chrono::DateTime<Utc>,
        end: Option<chrono::DateTime<Utc>>,
        billable_override: Option<bool>,
    ) -> Result<WorkSession, TimerDbError> {
        // Billable default: from the project (or `false`
        // when no project link); explicit override wins.
        let billable_default = if let Some(pid) = req.project_id {
            self.project_defaults
                .lookup(pid)
                .await
                .is_some_and(|(_, _, b)| b)
        } else {
            false
        };
        let billable = billable_override.unwrap_or(billable_default);

        let (rate_cents, currency) = if let Some(end_time) = end {
            // Closed session — snapshot the cascade right
            // now. `_end_time` quiets unused.
            let _ = end_time;
            let r = self
                .cascade_for(req.user_id, req.org_id, req.project_id)
                .await?;
            if billable {
                (r.hourly_cents, r.currency)
            } else {
                (0, String::new())
            }
        } else {
            // Open timer — snapshot happens on stop.
            (0, String::new())
        };

        let id = Uuid::new_v4();
        let now = Utc::now();
        let active = WorkSessionActive {
            id: Set(id),
            org_id: Set(req.org_id),
            user_id: Set(req.user_id),
            project_id: Set(req.project_id),
            project_path: Set(req.project_path.clone()),
            description: Set(req.description.clone()),
            start_time: Set(start),
            end_time: Set(end),
            billable: Set(billable),
            rate_cents: Set(rate_cents),
            currency: Set(currency),
            task_note_path: Set(req.task_note_path.clone()),
            invoice_id: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        };
        let inserted = active.insert(&self.conn).await.map_err(|e| {
            // SQLite surfaces the partial-unique-index violation as a constraint failure.
            let msg = e.to_string();
            if msg.contains("UNIQUE") || msg.contains("ux_timer_work_sessions_user_active") {
                TimerDbError::AlreadyRunning {
                    user_id: req.user_id.to_string(),
                }
            } else {
                TimerDbError::from(e)
            }
        })?;
        Ok(model_to_session(inserted))
    }

    async fn close_active(&self, user_id: Uuid) -> Result<WorkSession, TimerDbError> {
        let Some(open) = self.read_active(user_id).await? else {
            return Err(TimerDbError::NoActiveTimer {
                user_id: user_id.to_string(),
            });
        };
        let now = Utc::now();
        let (rate_cents, currency) = if open.billable {
            let r = self
                .cascade_for(user_id, open.org_id, open.project_id)
                .await?;
            (r.hourly_cents, r.currency)
        } else {
            (0, String::new())
        };
        WorkSessionEntity::update_many()
            .col_expr(WorkSessionColumn::EndTime, Expr::value(now))
            .col_expr(WorkSessionColumn::RateCents, Expr::value(rate_cents))
            .col_expr(WorkSessionColumn::Currency, Expr::value(currency))
            .col_expr(WorkSessionColumn::UpdatedAt, Expr::value(now))
            .filter(WorkSessionColumn::Id.eq(open.id))
            .filter(WorkSessionColumn::EndTime.is_null())
            .exec(&self.conn)
            .await?;
        let row = WorkSessionEntity::find_by_id(open.id)
            .one(&self.conn)
            .await?
            .ok_or_else(|| TimerDbError::NotFound {
                kind: "work_session",
                id: open.id.to_string(),
            })?;
        Ok(model_to_session(row))
    }

    // ── Reports / filters ───────────────────────────────────

    /// `list_sessions` analogue used by tests + the future
    /// server-side reports endpoint. Honors the bundled
    /// `WorkSessionFilter` flags.
    pub async fn query_sessions(
        &self,
        filter: &timer_proto::WorkSessionFilter,
    ) -> Result<Vec<WorkSession>, TimerDbError> {
        let mut q = WorkSessionEntity::find();
        if let Some(uid) = filter.user_id {
            q = q.filter(WorkSessionColumn::UserId.eq(uid));
        }
        if let Some(pid) = filter.project_id {
            q = q.filter(WorkSessionColumn::ProjectId.eq(pid));
        }
        if let Some(since) = filter.since {
            q = q.filter(WorkSessionColumn::StartTime.gte(since));
        }
        if let Some(until) = filter.until {
            q = q.filter(WorkSessionColumn::StartTime.lt(until));
        }
        if let Some(b) = filter.billable {
            q = q.filter(WorkSessionColumn::Billable.eq(b));
        }
        if let Some(true) = filter.open {
            q = q.filter(WorkSessionColumn::EndTime.is_null());
        }
        if let Some(false) = filter.open {
            q = q.filter(WorkSessionColumn::EndTime.is_not_null());
        }
        let rows = q
            .order_by_asc(WorkSessionColumn::StartTime)
            .all(&self.conn)
            .await?;
        Ok(rows.into_iter().map(model_to_session).collect())
    }
}

// ── Trait impl ─────────────────────────────────────────────

impl TimerService for Store {
    async fn start_timer(&self, req: StartTimerRequest) -> Result<WorkSession, TimerError> {
        let now = Utc::now();
        let started = self.insert_session(&req, now, None, None).await?;
        self.publish(timer_proto::TimerEvent::Upserted(started.clone()));
        Ok(started)
    }

    async fn stop_timer(&self, user_id: Uuid) -> Result<WorkSession, TimerError> {
        let closed = self.close_active(user_id).await?;
        self.publish(timer_proto::TimerEvent::Upserted(closed.clone()));
        Ok(closed)
    }

    async fn active_timer(&self, user_id: Uuid) -> Result<Option<WorkSession>, TimerError> {
        self.read_active(user_id).await.map_err(Into::into)
    }

    async fn switch_timer(
        &self,
        req: StartTimerRequest,
    ) -> Result<(Option<WorkSession>, WorkSession), TimerError> {
        // Close-then-open. Atomicity here is "good enough" —
        // SQLite serializes per-connection writes; the open
        // session is closed before the new insert, so the
        // partial-unique-index never sees two open rows.
        let closed = match self.close_active(req.user_id).await {
            Ok(s) => Some(s),
            Err(TimerDbError::NoActiveTimer { .. }) => None,
            Err(e) => return Err(e.into()),
        };
        let now = Utc::now();
        let started = self
            .insert_session(&req, now, None, None)
            .await
            .map_err(TimerError::from)?;
        // Two events, in write order: the closed row (if any), then
        // the fresh open one — subscribers see the same sequence the
        // DB went through.
        if let Some(closed) = &closed {
            self.publish(timer_proto::TimerEvent::Upserted(closed.clone()));
        }
        self.publish(timer_proto::TimerEvent::Upserted(started.clone()));
        Ok((closed, started))
    }

    async fn log_session(&self, req: LogSessionRequest) -> Result<WorkSession, TimerError> {
        if req.end_time <= req.start_time {
            return Err(TimerError::Invalid(
                "log_session: end_time must be > start_time".into(),
            ));
        }
        // Logged-in-the-past entries skip the active-timer
        // invariant — they have a non-null `end_time`, so
        // the partial unique index doesn't apply.
        let synthetic = StartTimerRequest {
            user_id: req.user_id,
            org_id: req.org_id,
            project_id: req.project_id,
            project_path: req.project_path,
            task_note_path: req.task_note_path,
            description: req.description,
        };
        let logged = self
            .insert_session(
                &synthetic,
                req.start_time,
                Some(req.end_time),
                req.billable_override,
            )
            .await?;
        self.publish(timer_proto::TimerEvent::Upserted(logged.clone()));
        Ok(logged)
    }

    async fn resolve_rate(
        &self,
        user_id: Uuid,
        project_id: Option<Uuid>,
    ) -> Result<RateResolution, TimerError> {
        // Without an org_id passed in we can't query the
        // OrgMemberRate level — caller passes via session
        // path instead. For the bare lookup, fall through:
        // we honor the project-level levels and skip org.
        // The full resolution path is internal to start/stop.
        let mut inputs = CascadeInputs::default();
        if let Some(pid) = project_id {
            if let Some(rate_row) = ProjectMemberRateEntity::find()
                .filter(ProjectMemberRateColumn::ProjectId.eq(pid))
                .filter(ProjectMemberRateColumn::UserId.eq(user_id))
                .one(&self.conn)
                .await
                .map_err(|e| TimerError::Backend(e.to_string()))?
            {
                let currency = self
                    .project_defaults
                    .lookup(pid)
                    .await
                    .map(|(_, c, _)| c)
                    .unwrap_or_default();
                inputs.project_member = Some((rate_row.hourly_cents, currency));
            }
            if let Some((cents, currency, _)) = self.project_defaults.lookup(pid).await {
                inputs.project_default = Some((cents, currency));
            }
        }
        let r = resolve(&inputs);
        Ok(RateResolution {
            hourly_cents: r.hourly_cents,
            currency: r.currency,
            source: r.source,
        })
    }

    async fn list_sessions(
        &self,
        filter: WorkSessionFilter,
    ) -> Result<Vec<WorkSession>, TimerError> {
        self.query_sessions(&filter).await.map_err(Into::into)
    }

    async fn update_session(
        &self,
        req: timer_proto::service::UpdateSessionRequest,
    ) -> Result<WorkSession, TimerError> {
        let row = WorkSessionEntity::find_by_id(req.id)
            .one(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?
            .ok_or_else(|| TimerError::WorkSessionNotFound(req.id.to_string()))?;

        // Resolve the final values (Some overwrites, None keeps). A
        // project can be set but not cleared through this path.
        let user_id = req.user_id.unwrap_or(row.user_id);
        let project_id = req.project_id.or(row.project_id);
        let start_time = req.start_time.unwrap_or(row.start_time);
        let end_time = req.end_time.or(row.end_time);
        let billable = req.billable.unwrap_or(row.billable);
        if let Some(et) = end_time {
            if et <= start_time {
                return Err(TimerError::Invalid(
                    "update_session: end_time must be > start_time".into(),
                ));
            }
        }

        // Re-snapshot the rate against the (possibly changed) user /
        // project / billable. Open or non-billable sessions hold $0.
        // `preserve_rate` keeps the stored snapshot untouched —
        // historical corrections (user reassignment) must not shift
        // already-billed amounts.
        let (rate_cents, currency) = if req.preserve_rate {
            (row.rate_cents, row.currency.clone())
        } else if end_time.is_some() && billable {
            let r = self
                .cascade_for(user_id, row.org_id, project_id)
                .await
                .map_err(TimerError::from)?;
            (r.hourly_cents, r.currency)
        } else {
            (0, String::new())
        };

        let mut active: WorkSessionActive = row.into();
        active.user_id = Set(user_id);
        active.project_id = Set(project_id);
        if let Some(p) = req.project_path {
            active.project_path = Set(p);
        }
        if let Some(t) = req.task_note_path {
            active.task_note_path = Set(t);
        }
        if let Some(d) = req.description {
            active.description = Set(d);
        }
        active.start_time = Set(start_time);
        active.end_time = Set(end_time);
        active.billable = Set(billable);
        active.rate_cents = Set(rate_cents);
        active.currency = Set(currency);
        active.updated_at = Set(Utc::now());
        let updated = active
            .update(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        let session = model_to_session(updated);
        self.publish(timer_proto::TimerEvent::Upserted(session.clone()));
        Ok(session)
    }

    async fn delete_session(&self, id: Uuid) -> Result<(), TimerError> {
        WorkSessionEntity::delete_by_id(id)
            .exec(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        self.publish(timer_proto::TimerEvent::Deleted(id));
        Ok(())
    }

    async fn set_org_member_rate(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        hourly_cents: i64,
        currency: String,
    ) -> Result<OrgMemberRate, TimerError> {
        // Reuse the inherent upsert helper (takes &str), read the row back.
        Store::set_org_member_rate(self, org_id, user_id, hourly_cents, &currency)
            .await
            .map_err(TimerError::from)?;
        let row = OrgMemberRateEntity::find()
            .filter(OrgMemberRateColumn::OrgId.eq(org_id))
            .filter(OrgMemberRateColumn::UserId.eq(user_id))
            .one(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?
            .ok_or_else(|| TimerError::Backend("org member rate vanished after upsert".into()))?;
        Ok(org_member_rate_from_model(row))
    }

    async fn set_project_member_rate(
        &self,
        project_id: Uuid,
        user_id: Uuid,
        hourly_cents: i64,
    ) -> Result<ProjectMemberRate, TimerError> {
        let existing = ProjectMemberRateEntity::find()
            .filter(ProjectMemberRateColumn::ProjectId.eq(project_id))
            .filter(ProjectMemberRateColumn::UserId.eq(user_id))
            .one(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        let now = Utc::now();
        let saved = if let Some(row) = existing {
            let mut a: ProjectMemberRateActive = row.into();
            a.hourly_cents = Set(hourly_cents);
            a.updated_at = Set(now);
            a.update(&self.conn).await
        } else {
            ProjectMemberRateActive {
                id: Set(Uuid::new_v4()),
                project_id: Set(project_id),
                user_id: Set(user_id),
                hourly_cents: Set(hourly_cents),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&self.conn)
            .await
        }
        .map_err(|e| TimerError::Backend(e.to_string()))?;
        Ok(project_member_rate_from_model(saved))
    }

    async fn list_org_member_rates(&self, org_id: Uuid) -> Result<Vec<OrgMemberRate>, TimerError> {
        let rows = OrgMemberRateEntity::find()
            .filter(OrgMemberRateColumn::OrgId.eq(org_id))
            .all(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        Ok(rows.into_iter().map(org_member_rate_from_model).collect())
    }

    async fn list_project_member_rates(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<ProjectMemberRate>, TimerError> {
        let rows = ProjectMemberRateEntity::find()
            .filter(ProjectMemberRateColumn::ProjectId.eq(project_id))
            .all(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(project_member_rate_from_model)
            .collect())
    }

    async fn list_tags(&self, org_id: Uuid) -> Result<Vec<Tag>, TimerError> {
        let rows = crate::entity::TagEntity::find()
            .filter(crate::entity::TagColumn::OrgId.eq(org_id))
            .all(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        Ok(rows.into_iter().map(Tag::from).collect())
    }

    async fn create_tag(
        &self,
        org_id: Uuid,
        name: String,
        color: String,
    ) -> Result<Tag, TimerError> {
        self.ensure_tag(org_id, &name, &color).await.map(Tag::from)
    }

    async fn delete_tag(&self, org_id: Uuid, name: String) -> Result<Tag, TimerError> {
        let existing = crate::entity::TagEntity::find()
            .filter(crate::entity::TagColumn::OrgId.eq(org_id))
            .filter(crate::entity::TagColumn::Name.eq(name.clone()))
            .one(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?
            .ok_or(TimerError::TagNotFound(name))?;
        crate::entity::TagEntity::delete_by_id(existing.id)
            .exec(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        Ok(Tag::from(existing))
    }

    async fn attach_tags(
        &self,
        session_id: Uuid,
        org_id: Uuid,
        names: Vec<String>,
    ) -> Result<(), TimerError> {
        for name in &names {
            let tag = self.ensure_tag(org_id, name, "").await?;
            let already = crate::entity::WorkSessionTagEntity::find()
                .filter(crate::entity::WorkSessionTagColumn::WorkSessionId.eq(session_id))
                .filter(crate::entity::WorkSessionTagColumn::TagId.eq(tag.id))
                .one(&self.conn)
                .await
                .map_err(|e| TimerError::Backend(e.to_string()))?;
            if already.is_some() {
                continue;
            }
            crate::entity::WorkSessionTagActive {
                id: Set(Uuid::new_v4()),
                work_session_id: Set(session_id),
                tag_id: Set(tag.id),
                created_at: Set(Utc::now()),
            }
            .insert(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        }
        Ok(())
    }

    async fn detach_tags(
        &self,
        session_id: Uuid,
        org_id: Uuid,
        names: Vec<String>,
        all: bool,
    ) -> Result<u64, TimerError> {
        if all {
            crate::entity::WorkSessionTagEntity::delete_many()
                .filter(crate::entity::WorkSessionTagColumn::WorkSessionId.eq(session_id))
                .exec(&self.conn)
                .await
                .map_err(|e| TimerError::Backend(e.to_string()))?;
            return Ok(0);
        }
        let tag_rows = crate::entity::TagEntity::find()
            .filter(crate::entity::TagColumn::OrgId.eq(org_id))
            .filter(crate::entity::TagColumn::Name.is_in(names))
            .all(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?;
        let matched = tag_rows.len() as u64;
        let ids: Vec<Uuid> = tag_rows.iter().map(|t| t.id).collect();
        if !ids.is_empty() {
            crate::entity::WorkSessionTagEntity::delete_many()
                .filter(crate::entity::WorkSessionTagColumn::WorkSessionId.eq(session_id))
                .filter(crate::entity::WorkSessionTagColumn::TagId.is_in(ids))
                .exec(&self.conn)
                .await
                .map_err(|e| TimerError::Backend(e.to_string()))?;
        }
        Ok(matched)
    }
}

impl Store {
    /// Idempotent tag upsert by `(org_id, name)` — the shared core of
    /// `create_tag` / `attach_tags`. An existing row wins untouched
    /// (its color is not updated).
    async fn ensure_tag(
        &self,
        org_id: Uuid,
        name: &str,
        color: &str,
    ) -> Result<crate::entity::TagModel, TimerError> {
        if let Some(existing) = crate::entity::TagEntity::find()
            .filter(crate::entity::TagColumn::OrgId.eq(org_id))
            .filter(crate::entity::TagColumn::Name.eq(name.to_string()))
            .one(&self.conn)
            .await
            .map_err(|e| TimerError::Backend(e.to_string()))?
        {
            return Ok(existing);
        }
        let now = Utc::now();
        crate::entity::TagActive {
            id: Set(Uuid::new_v4()),
            org_id: Set(org_id),
            name: Set(name.to_string()),
            color: Set(color.to_string()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&self.conn)
        .await
        .map_err(|e| TimerError::Backend(e.to_string()))
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `TimerService` impl above, on every successful session mutation.
impl timer_proto::TimerServiceStreamSource for Store {
    fn events_hub(&self) -> &architect::PubSub<timer_proto::TimerEvent> {
        &self.events
    }
}

fn org_member_rate_from_model(m: OrgMemberRateModel) -> OrgMemberRate {
    OrgMemberRate {
        id: m.id,
        org_id: m.org_id,
        user_id: m.user_id,
        hourly_cents: m.hourly_cents,
        currency: m.currency,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn project_member_rate_from_model(m: ProjectMemberRateModel) -> ProjectMemberRate {
    ProjectMemberRate {
        id: m.id,
        project_id: m.project_id,
        user_id: m.user_id,
        hourly_cents: m.hourly_cents,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn model_to_session(m: WorkSessionModel) -> WorkSession {
    WorkSession {
        id: m.id,
        org_id: m.org_id,
        user_id: m.user_id,
        project_id: m.project_id,
        project_path: m.project_path,
        description: m.description,
        start_time: m.start_time,
        end_time: m.end_time,
        billable: m.billable,
        rate_cents: m.rate_cents,
        currency: m.currency,
        task_note_path: m.task_note_path,
        invoice_id: m.invoice_id,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

#[allow(dead_code)]
const _UNUSED_RATE_SOURCE: RateSource = RateSource::None;
