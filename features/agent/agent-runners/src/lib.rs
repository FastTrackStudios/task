//! The runner registry — where a runner registers, and how the
//! server knows whether it is still alive.
//!
//! A **runner** is an [`AgentBackend`] that declares a
//! [`RunnerProfile`]: what it can do, which orgs and projects it
//! serves, and how many tickets it will hold at once. This crate
//! stores those registrations and implements the [`Backends`] RPC
//! trait, which until now was a wire contract with nothing behind
//! it.
//!
//! # Storage
//!
//! One table, `agent_backends`, holding the registration as JSON
//! beside the two columns that are queried — `id` and `kind`.
//!
//! The profile is a small, nested, rapidly-evolving shape
//! (capabilities are a closed enum with a payload, scope is two
//! lists). Spreading that across typed columns would mean a
//! migration every time the vocabulary grows, to buy filtering
//! nobody does: the registry is read whole, on every routing
//! decision, and holds one row per machine. JSON is the right
//! trade here, and `kind` is lifted out because
//! [`Backends::backends_by_kind`] is the one query that filters.
//!
//! # Liveness
//!
//! Registration persists; liveness does not. A runner heartbeats,
//! and a runner whose last heartbeat is older than
//! [`STALE_AFTER`] is reported unhealthy — so a rebooted machine
//! stops being offered work without anyone deregistering it.

use std::time::Duration;

use agent_proto::backend::{AgentBackend, BackendHealth, BackendKind};
use agent_proto::error::AgentError;
use agent_proto::runner::{Capability, RunnerProfile, TicketRequirements};
use agent_proto::service::backends::Backends;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};

pub mod migrations;
pub mod questions;
pub mod runs;

pub use migrations::Migrator;
pub use questions::QuestionStore;
pub use runs::{RUN_STALE_AFTER, RunStore};

/// How long after its last heartbeat a runner is considered stale.
///
/// Generous on purpose: a runner mid-build can be slow to check in,
/// and wrongly declaring a working machine dead costs more than
/// noticing a dead one a minute late.
pub const STALE_AFTER: Duration = Duration::from_secs(120);

/// Server-side runner registry.
#[derive(Clone, Debug)]
pub struct Store {
    conn: DatabaseConnection,
}

/// A registration plus whether it is currently alive.
#[derive(Debug, Clone, PartialEq)]
pub struct Registration {
    pub backend: AgentBackend,
    /// `true` when the last heartbeat is within [`STALE_AFTER`].
    pub live: bool,
}

impl Store {
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    /// Register or update a runner.
    ///
    /// Validates the capability list against the closed vocabulary
    /// before anything is written — a runner that believes it
    /// advertised a capability it did not is worse than one that
    /// failed to start.
    ///
    /// # Errors
    ///
    /// [`AgentError`] on a bad capability or a storage failure.
    pub async fn upsert(&self, backend: AgentBackend) -> Result<AgentBackend, AgentError> {
        validate(&backend)?;
        let json = serde_json::to_string(&backend)
            .map_err(|e| AgentError::Backend(format!("encode backend: {e}")))?;
        let kind = format!("{:?}", backend.kind);
        let stmt = Statement::from_sql_and_values(
            self.conn.get_database_backend(),
            "INSERT INTO agent_backends (id, kind, json) VALUES (?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, json = excluded.json",
            [backend.id.clone().into(), kind.into(), json.into()],
        );
        self.conn
            .execute(stmt)
            .await
            .map_err(|e| AgentError::Backend(format!("upsert backend: {e}")))?;
        Ok(backend)
    }

    /// Record that a runner is alive right now.
    ///
    /// # Errors
    ///
    /// [`AgentError::BackendNotFound`] when the runner is not registered.
    pub async fn heartbeat(&self, backend_id: &str) -> Result<(), AgentError> {
        let mut backend = self.get(backend_id).await?;
        backend.last_seen = Some(Utc::now());
        self.upsert(backend).await?;
        Ok(())
    }

    /// One registration.
    ///
    /// # Errors
    ///
    /// [`AgentError::BackendNotFound`] when it is not registered.
    pub async fn get(&self, backend_id: &str) -> Result<AgentBackend, AgentError> {
        self.list()
            .await?
            .into_iter()
            .find(|b| b.id == backend_id)
            .ok_or_else(|| AgentError::BackendNotFound(backend_id.to_string()))
    }

    /// Every registered runner.
    ///
    /// # Errors
    ///
    /// [`AgentError`] on a storage failure. A row that fails to
    /// decode is skipped and logged rather than failing the whole
    /// listing — one bad registration must not blind the router to
    /// every other machine.
    pub async fn list(&self) -> Result<Vec<AgentBackend>, AgentError> {
        let rows = self
            .conn
            .query_all(Statement::from_string(
                self.conn.get_database_backend(),
                "SELECT json FROM agent_backends ORDER BY id",
            ))
            .await
            .map_err(|e| AgentError::Backend(format!("list backends: {e}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let json: String = row
                .try_get("", "json")
                .map_err(|e| AgentError::Backend(format!("read backend row: {e}")))?;
            match serde_json::from_str::<AgentBackend>(&json) {
                Ok(b) => out.push(b),
                Err(e) => tracing::warn!(error = %e, "skipping undecodable runner registration"),
            }
        }
        Ok(out)
    }

    /// Every registration with its liveness, as of now.
    ///
    /// # Errors
    ///
    /// [`AgentError`] on a storage failure.
    pub async fn registrations(&self) -> Result<Vec<Registration>, AgentError> {
        let now = Utc::now();
        Ok(self
            .list()
            .await?
            .into_iter()
            .map(|backend| Registration {
                live: is_live(backend.last_seen, now),
                backend,
            })
            .collect())
    }

    /// Deregister a runner.
    ///
    /// # Errors
    ///
    /// [`AgentError`] on a storage failure.
    pub async fn remove(&self, backend_id: &str) -> Result<(), AgentError> {
        let stmt = Statement::from_sql_and_values(
            self.conn.get_database_backend(),
            "DELETE FROM agent_backends WHERE id = ?",
            [backend_id.into()],
        );
        self.conn
            .execute(stmt)
            .await
            .map_err(|e| AgentError::Backend(format!("remove backend: {e}")))?;
        Ok(())
    }

    /// The profiles routing should consider — live runners only.
    ///
    /// A stale runner keeps its registration but is not offered
    /// work, which is what makes a reboot self-healing.
    ///
    /// # Errors
    ///
    /// [`AgentError`] on a storage failure.
    pub async fn routable(&self) -> Result<Vec<RunnerProfile>, AgentError> {
        Ok(self
            .registrations()
            .await?
            .into_iter()
            .filter(|r| r.live)
            .map(|r| r.backend.runner)
            .collect())
    }

    /// The capability no live runner offers for this ticket, if any.
    ///
    /// # Errors
    ///
    /// [`AgentError`] on a storage failure.
    pub async fn unroutable_reason(
        &self,
        req: &TicketRequirements,
    ) -> Result<Option<String>, AgentError> {
        let live = self.routable().await?;
        Ok(agent_proto::runner::unsatisfiable_capability(req, &live))
    }
}

/// Is a runner with this last-heartbeat live as of `now`?
fn is_live(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let Some(seen) = last_seen else {
        return false;
    };
    match now.signed_duration_since(seen).to_std() {
        Ok(elapsed) => elapsed <= STALE_AFTER,
        // A heartbeat in the future (clock skew) counts as live.
        Err(_) => true,
    }
}

/// Reject a registration whose capability list is not in the closed
/// vocabulary.
fn validate(backend: &AgentBackend) -> Result<(), AgentError> {
    for cap in &backend.runner.capabilities {
        // Round-tripping through the parser is the check: a variant
        // that cannot be re-parsed from its own wire form is not in
        // the vocabulary.
        let text = cap.as_string();
        Capability::parse(&text)
            .map_err(|e| AgentError::Invalid(e.to_string()))?;
    }
    if backend.runner.id != backend.id {
        return Err(AgentError::Invalid(format!(
            "runner profile id `{}` does not match backend id `{}`",
            backend.runner.id, backend.id
        )));
    }
    Ok(())
}

impl Backends for Store {
    async fn upsert_backend(&self, backend: AgentBackend) -> Result<AgentBackend, AgentError> {
        self.upsert(backend).await
    }

    async fn remove_backend(&self, backend_id: String) -> Result<(), AgentError> {
        self.remove(&backend_id).await
    }

    async fn list_backends(&self) -> Result<Vec<AgentBackend>, AgentError> {
        self.list().await
    }

    async fn backend_health(&self, backend_id: String) -> Result<BackendHealth, AgentError> {
        let backend = self.get(&backend_id).await?;
        let live = is_live(backend.last_seen, Utc::now());
        Ok(BackendHealth {
            backend_id: backend.id,
            reachable: live,
            last_ping_ms: 0,
            version: String::new(),
            status_text: if live {
                String::new()
            } else {
                "no heartbeat within the stale window".into()
            },
            state: if live { "running".into() } else { "stale".into() },
            active_agents: 0,
            platforms: Vec::new(),
            model: String::new(),
            at: Utc::now(),
        })
    }

    async fn heartbeat_backend(&self, backend_id: String) -> Result<(), AgentError> {
        self.heartbeat(&backend_id).await
    }

    async fn backends_by_kind(&self, kind: BackendKind) -> Result<Vec<AgentBackend>, AgentError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .filter(|b| b.kind == kind)
            .collect())
    }
}
