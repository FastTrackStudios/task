//! Server-native git snapshots — the `SnapshotService` impl, the
//! write gate, and the CronJob-facing HTTP trigger.
//!
//! Replaces the chart's git-in-a-CronJob script with an in-process
//! cycle the server itself runs, so the committed sqlite files are
//! *consistent* instead of best-effort live copies:
//!
//! 1. **Quiesce** — [`WriteGate::pause`] holds the global gate;
//!    every vox request (per-org and `/server/vox`) waits at entry
//!    via [`GatedRouter`] until the gate reopens. In-flight
//!    requests are not interrupted; the gate closes the door,
//!    sqlite's own locking serializes the stragglers.
//! 2. **Checkpoint** — `PRAGMA wal_checkpoint(TRUNCATE)` on every
//!    open sqlite pool ([`crate::OrgAppState::sqlite_conns`]), so
//!    the main db files contain everything and the WAL sidecars
//!    are empty.
//! 3. **Commit** — `org-snapshot` commits the per-org repos + the
//!    full-state repo (same `.gitstate/` layout the script used in
//!    production; histories continue seamlessly).
//! 4. **Release + push** — the gate reopens before the slow network
//!    part; commits are immutable so the push needs no quiesce.
//!
//! Verbs are mounted at `/server/vox` next to
//! `OrgManagementService`; `POST /server/snapshot` (Bearer
//! `TASK_BACKUP_GIT_TOKEN`) triggers the same cycle for the chart's
//! CronJob.

use std::sync::Arc;

use org_proto::snapshot::{
    BranchResult, RepoResult, RestoreReport, SnapshotError, SnapshotLogEntry, SnapshotReport,
    SnapshotService,
};
use org_snapshot::SnapshotEngine;
use sea_orm::ConnectionTrait;

use crate::{AppState, OrgAppState};

// ── Write gate ────────────────────────────────────────────────────

/// Global write quiesce for snapshot cycles.
///
/// Request handlers call [`WriteGate::wait_open`] at dispatch entry:
/// when no snapshot is running this is a fetch-and-drop on an
/// uncontended `RwLock` (cheap); while a snapshot holds
/// [`WriteGate::pause`], new requests park until the commit phase
/// finishes. The guard is **not** held across the request body —
/// long-lived subscribe streams would otherwise pin the gate open
/// forever and deadlock the snapshot.
#[derive(Clone, Default)]
pub struct WriteGate(Arc<tokio::sync::RwLock<()>>);

impl WriteGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Park until no snapshot holds the gate. Does not hold anything
    /// on return.
    pub async fn wait_open(&self) {
        drop(self.0.read().await);
    }

    /// Close the gate. New vox requests wait at entry until the
    /// returned guard drops.
    pub async fn pause(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.0.write().await
    }
}

// ── Gated router ──────────────────────────────────────────────────

/// [`architect::LayerRouter`] wrapper that parks every incoming
/// request at the [`WriteGate`] before dispatching. Mounted on every
/// vox endpoint (per-org + `/server/vox`) so snapshot cycles see a
/// quiesced write path.
#[derive(Clone)]
pub struct GatedRouter {
    inner: architect::LayerRouter,
    gate: WriteGate,
}

impl GatedRouter {
    #[must_use]
    pub fn new(inner: architect::LayerRouter, gate: WriteGate) -> Self {
        Self { inner, gate }
    }
}

impl vox::Handler<vox::DriverReplySink> for GatedRouter {
    fn args_have_channels(&self, method_id: vox::MethodId) -> bool {
        self.inner.args_have_channels(method_id)
    }

    fn response_wire_shape(&self, method_id: vox::MethodId) -> Option<&'static facet::Shape> {
        self.inner.response_wire_shape(method_id)
    }

    async fn handle(
        &self,
        call: vox::SelfRef<vox::RequestCall<'static>>,
        reply: vox::DriverReplySink,
        schemas: Arc<vox::SchemaRecvTracker>,
    ) {
        // Entry gate only — never held across the (possibly
        // streaming, possibly long-lived) request body.
        self.gate.wait_open().await;
        self.inner.handle(call, reply, schemas).await;
    }
}

// ── Cycle orchestration ───────────────────────────────────────────

/// `PRAGMA wal_checkpoint(TRUNCATE)` on every open sqlite pool of
/// every hosted org. Best-effort per handle: a failed checkpoint is
/// logged and the cycle continues (the main db file is still
/// consistent in WAL mode — it just won't contain the un-checkpointed
/// tail).
async fn checkpoint_all(state: &AppState) {
    let orgs: Vec<OrgAppState> = state
        .orgs
        .read()
        .map(|g| g.values().cloned().collect())
        .unwrap_or_default();
    for org in orgs {
        for conn in &org.sqlite_conns {
            if let Err(e) = conn
                .execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE);")
                .await
            {
                tracing::warn!(org = %org.slug, error = %e, "wal_checkpoint failed");
            }
        }
    }
}

fn internal(e: org_snapshot::EngineError) -> SnapshotError {
    match e {
        org_snapshot::EngineError::Refused(m) => SnapshotError::Refused(m),
        org_snapshot::EngineError::Git { verb, detail } => {
            SnapshotError::Git(format!("{verb}: {detail}"))
        }
        other => SnapshotError::Internal(other.to_string()),
    }
}

/// One full snapshot cycle against `state`'s data root. Assumes the
/// caller already holds `state.snapshot_cycle` (cycles are
/// serialized).
async fn run_cycle_locked(
    state: &AppState,
    engine: &SnapshotEngine,
) -> Result<SnapshotReport, SnapshotError> {
    // Quiesce → checkpoint → commit, all under the gate. Local-only
    // work; the slow push happens after release.
    let guard = state.write_gate.pause().await;
    checkpoint_all(state).await;
    let (stamp, repos) = engine.commit_phase().await.map_err(internal)?;
    drop(guard);
    let repos = engine.push_phase(repos).await;
    for r in &repos {
        if r.error.is_empty() {
            tracing::info!(repo = %r.repo, committed = %r.committed, clean = r.clean, pushed = r.pushed, "snapshot repo");
        } else {
            tracing::warn!(repo = %r.repo, error = %r.error, "snapshot repo failed");
        }
    }
    Ok(SnapshotReport { stamp, repos })
}

/// Public entry: serialize on `state.snapshot_cycle`, then run a
/// cycle. `Busy` when another cycle (or a restore) is in flight.
pub async fn run_cycle(state: &AppState) -> Result<SnapshotReport, SnapshotError> {
    let _cycle = state
        .snapshot_cycle
        .try_lock()
        .map_err(|_| SnapshotError::Busy("a snapshot/restore cycle is already running".into()))?;
    let engine = SnapshotEngine::from_env(state.data_root.path());
    run_cycle_locked(state, &engine).await
}

// ── SnapshotService impl ──────────────────────────────────────────

/// `SnapshotService` backend mounted at `/server/vox`.
#[derive(Clone, architect::HasDispatcher)]
pub struct SnapshotImpl {
    state: Arc<AppState>,
    /// Production: exit(0) after a successful restore so the
    /// supervisor restarts the server on the restored data. Tests
    /// turn this off via [`SnapshotImpl::new_without_exit`].
    exit_on_restore: bool,
    /// True when served over the in-process `LocalServer` (embedded
    /// CLI): the caller already owns the data root on disk, so
    /// [`Self::authorize`] passes — same trust model as the per-org
    /// embedded transport.
    local_trusted: bool,
}

impl SnapshotImpl {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            exit_on_restore: true,
            local_trusted: false,
        }
    }

    /// Test constructor — restore mutates the work tree but the
    /// process stays alive.
    #[must_use]
    pub fn new_without_exit(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            exit_on_restore: false,
            local_trusted: false,
        }
    }

    /// In-process transport constructor (embedded CLI): trusted (no
    /// session validation) and no exit-on-restore — the CLI process
    /// is ephemeral and exits after the verb anyway.
    #[must_use]
    pub fn new_local_trusted(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
            exit_on_restore: false,
            local_trusted: true,
        }
    }

    /// Authorize a snapshot verb:
    /// - the configured backup token (`TASK_BACKUP_GIT_TOKEN`) is
    ///   always accepted — non-interactive automation;
    /// - bootstrap mode (no orgs hosted) is permissive, mirroring
    ///   `OrgManagementService::create_org`;
    /// - otherwise the token must be a valid session against the
    ///   home org.
    async fn authorize(&self, token: &str) -> Result<(), SnapshotError> {
        if self.local_trusted {
            return Ok(());
        }
        if let Ok(expected) = std::env::var("TASK_BACKUP_GIT_TOKEN") {
            if !expected.is_empty() && token == expected {
                return Ok(());
            }
        }
        if self.state.is_bootstrap() {
            return Ok(());
        }
        let home_slug = self.state.home_slug().ok_or_else(|| {
            SnapshotError::Unauthorized("server has orgs but no home org — cannot validate".into())
        })?;
        if token.is_empty() {
            return Err(SnapshotError::Unauthorized(
                "missing session token (sign in to the home org, or use the backup token)".into(),
            ));
        }
        let home = self.state.org(&home_slug).ok_or_else(|| {
            SnapshotError::Unauthorized(format!("home org `{home_slug}` not in live dispatcher"))
        })?;
        home.auth
            .auth
            .current_session(architect_auth::commands::CurrentSession {
                token: token.to_owned(),
            })
            .await
            .map_err(|e| SnapshotError::Unauthorized(format!("invalid session token: {e}")))?;
        Ok(())
    }

    fn engine(&self) -> SnapshotEngine {
        SnapshotEngine::from_env(self.state.data_root.path())
    }
}

impl SnapshotService for SnapshotImpl {
    async fn snapshot(&self, session_token: String) -> Result<SnapshotReport, SnapshotError> {
        self.authorize(&session_token).await?;
        run_cycle(&self.state).await
    }

    async fn log(
        &self,
        session_token: String,
        limit: u32,
    ) -> Result<Vec<SnapshotLogEntry>, SnapshotError> {
        self.authorize(&session_token).await?;
        self.engine().log(limit).await.map_err(internal)
    }

    async fn branch(
        &self,
        session_token: String,
        name: String,
    ) -> Result<BranchResult, SnapshotError> {
        self.authorize(&session_token).await?;
        let _cycle = self.state.snapshot_cycle.try_lock().map_err(|_| {
            SnapshotError::Busy("a snapshot/restore cycle is already running".into())
        })?;
        self.engine().branch(&name).await.map_err(internal)
    }

    async fn restore(
        &self,
        session_token: String,
        commit: String,
        confirm: String,
        force: bool,
    ) -> Result<RestoreReport, SnapshotError> {
        self.authorize(&session_token).await?;
        if confirm != commit {
            return Err(SnapshotError::Refused(format!(
                "confirmation mismatch — pass confirm equal to the commit (`{commit}`)"
            )));
        }
        // Hold the cycle lock across rescue-snapshot + checkout so a
        // concurrent snapshot can't interleave.
        let _cycle = self.state.snapshot_cycle.try_lock().map_err(|_| {
            SnapshotError::Busy("a snapshot/restore cycle is already running".into())
        })?;
        let engine = self.engine();

        let mut pre_restore: Vec<RepoResult> = Vec::new();
        if force {
            if engine.full_tree_dirty().await.unwrap_or(true) {
                tracing::warn!(
                    "forced restore over a dirty work tree — pre-restore state is NOT committed"
                );
            }
        } else {
            // Default path: rescue snapshot first. The tree is clean
            // afterwards and the pre-restore state is a commit we
            // can roll forward to. A failed rescue aborts.
            let report = run_cycle_locked(&self.state, &engine).await.map_err(|e| {
                SnapshotError::Refused(format!(
                    "rescue snapshot failed — refusing to restore without `force` ({e})"
                ))
            })?;
            if let Some(bad) = report.repos.iter().find(|r| !r.error.is_empty()) {
                return Err(SnapshotError::Refused(format!(
                    "rescue snapshot failed on `{}` — refusing to restore without `force` ({})",
                    bad.repo, bad.error
                )));
            }
            pre_restore = report.repos;
        }

        // Gate writes during the checkout itself.
        let guard = self.state.write_gate.pause().await;
        let sha = engine.restore_checkout(&commit).await.map_err(internal)?;
        drop(guard);

        if self.exit_on_restore {
            tracing::warn!(
                commit = %sha,
                "data root restored — exiting so the supervisor restarts the server on the \
                 restored data (local dev: restart task-server manually)"
            );
            tokio::spawn(async {
                // Let the reply flush before the process dies.
                tokio::time::sleep(std::time::Duration::from_millis(750)).await;
                std::process::exit(0);
            });
        } else {
            tracing::warn!(commit = %sha, "data root restored (exit-on-restore disabled)");
        }
        Ok(RestoreReport {
            commit: sha,
            pre_restore,
            restarting: self.exit_on_restore,
        })
    }
}

// ── Async status ──────────────────────────────────────────────────

/// Phase of the most recent async snapshot kick-off.
#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotPhase {
    /// No async snapshot has been started this process lifetime.
    #[default]
    Idle,
    /// A cycle kicked off via `?wait=0` is running.
    Running,
    /// The last async cycle finished (see `repos` for per-repo results).
    Done,
    /// The last async cycle failed before producing a report.
    Failed,
}

/// Pollable status for the async snapshot trigger
/// (`GET /server/snapshot/status`). The synchronous trigger and the
/// vox `snapshot()` verb don't touch this — it tracks only the
/// `POST /server/snapshot?wait=0` kick-off.
#[derive(Clone, Default, serde::Serialize)]
pub struct SnapshotStatus {
    pub phase: SnapshotPhase,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub stamp: Option<String>,
    pub repos: Vec<serde_json::Value>,
    pub error: Option<String>,
}

fn report_repos_json(report: &SnapshotReport) -> Vec<serde_json::Value> {
    report
        .repos
        .iter()
        .map(|r| {
            serde_json::json!({
                "repo": r.repo,
                "committed": r.committed,
                "clean": r.clean,
                "pushed": r.pushed,
                "error": r.error,
            })
        })
        .collect()
}

// ── HTTP trigger (CronJob-facing) ─────────────────────────────────

/// `POST /server/snapshot` — run one snapshot cycle. Auth:
/// `Authorization: Bearer <TASK_BACKUP_GIT_TOKEN>`. 503 when no
/// backup token is configured (the endpoint is then disabled), 401
/// on a bad token, 409 when a cycle is already running.
///
/// **Synchronous by default** (the in-cluster CronJob has no edge
/// timeout): runs the full cycle and returns the report. With
/// **`?wait=0`** it kicks the cycle off on a background task and
/// returns `202 Accepted` immediately — for triggers behind a proxy
/// (Cloudflare) whose request timeout is shorter than a full
/// snapshot. Poll [`http_snapshot_status_handler`] for completion.
pub async fn http_snapshot_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    if let Err(resp) = check_backup_auth(&headers) {
        return *resp;
    }

    // `?wait=0` ⇒ kick off + return 202. Anything else ⇒ synchronous.
    let async_kick = query
        .as_deref()
        .is_some_and(|q| q.split('&').any(|kv| kv == "wait=0"));

    if async_kick {
        return kick_off_async(state).into_response();
    }

    match run_cycle(&state).await {
        Ok(report) => axum::Json(serde_json::json!({
            "stamp": report.stamp,
            "repos": report_repos_json(&report),
        }))
        .into_response(),
        Err(SnapshotError::Busy(m)) => (axum::http::StatusCode::CONFLICT, m).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /server/snapshot/status` — the last async snapshot's status.
/// Same bearer auth as the trigger.
pub async fn http_snapshot_status_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    if let Err(resp) = check_backup_auth(&headers) {
        return *resp;
    }
    let status = state.snapshot_status.read().unwrap().clone();
    axum::Json(status).into_response()
}

/// Spawn the cycle, return `202 Accepted` now. `409` if a cycle is
/// already in flight (the spawned task's first act — `run_cycle` —
/// `try_lock`s `snapshot_cycle`, but we also short-circuit here so
/// the caller gets an immediate, honest 409 instead of a phantom
/// "started").
fn kick_off_async(state: AppState) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    {
        let mut st = state.snapshot_status.write().unwrap();
        if st.phase == SnapshotPhase::Running {
            return (
                axum::http::StatusCode::CONFLICT,
                "a snapshot cycle is already running",
            )
                .into_response();
        }
        *st = SnapshotStatus {
            phase: SnapshotPhase::Running,
            started_at: Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            ..Default::default()
        };
    }

    let started_at = state.snapshot_status.read().unwrap().started_at.clone();

    tokio::spawn(async move {
        let outcome = run_cycle(&state).await;
        let finished_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut st = state.snapshot_status.write().unwrap();
        st.finished_at = Some(finished_at);
        match outcome {
            Ok(report) => {
                st.phase = SnapshotPhase::Done;
                st.stamp = Some(report.stamp.clone());
                st.repos = report_repos_json(&report);
            }
            Err(e) => {
                st.phase = SnapshotPhase::Failed;
                st.error = Some(e.to_string());
            }
        }
    });

    (
        axum::http::StatusCode::ACCEPTED,
        axum::Json(serde_json::json!({
            "phase": "running",
            "started_at": started_at,
            "poll": "GET /server/snapshot/status",
        })),
    )
        .into_response()
}

/// Shared bearer-token gate for the snapshot HTTP routes.
// Err is boxed: an axum `Response` is large, and `result_large_err`
// flags returning it by value (the Ok path is the common one).
pub(crate) fn check_backup_auth(
    headers: &axum::http::HeaderMap,
) -> Result<(), Box<axum::response::Response>> {
    use axum::response::IntoResponse as _;

    let expected = std::env::var("TASK_BACKUP_GIT_TOKEN").unwrap_or_default();
    if expected.is_empty() {
        return Err(Box::new(
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "snapshot endpoint disabled — TASK_BACKUP_GIT_TOKEN is not configured",
            )
                .into_response(),
        ));
    }
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if provided != expected {
        return Err(Box::new(
            (
                axum::http::StatusCode::UNAUTHORIZED,
                "bad or missing bearer token",
            )
                .into_response(),
        ));
    }
    Ok(())
}
