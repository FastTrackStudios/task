//! HTTP bridge for the watchOS Task app.
//!
//! watchOS forbids WebSockets outside audio sessions (Apple TN3135), so the
//! watch cannot speak vox. This mounts plain axum routes that call the org's
//! `timer` + `inbox` backends **in-process** — the same concrete backends the
//! vox layer wraps in [`crate::org_layer_router`]. It is the Task counterpart
//! of FTS's `engine_watch.rs`, following the `EngineHost::extend` precedent
//! architect ships for watch remotes.
//!
//! Auth accepts `Authorization: Bearer <token>` where the token is EITHER:
//!   1. a real architect-auth **session token** (validated via
//!      `current_session` → the session's `user.id`) — this is what a watch
//!      that inherited the phone's signed-in session presents; or
//!   2. the static **device token** `TASK_WATCH_TOKEN`, whose identity is the
//!      deterministic local owner (`user_id = v5(org_id, "task-local-owner")`,
//!      matching `task_ui::chrome::owner_id` / the CLI) — a headless/testing
//!      fallback.
//!      Either way the resulting `user_id` keys the timer/inbox rows so
//!      watch entries land in the same keyspace as the desktop and CLI.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use inbox_proto::{Inbox, InboxItem};
use timer_proto::{StartTimerRequest, TimerService};

/// Deterministic single-user owner id, so the watch, web UI, and CLI
/// all key timer rows on the same user. One definition, in
/// `timer_proto::owner`.
use timer_proto::local_owner_id as owner_id;

/// Extract the `Authorization: Bearer <token>` value, if present.
/// Shared with the other HTTP token gates (`crate::mcp`) — one
/// extraction, not per-module forks.
pub(crate) fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned)
        .filter(|t| !t.is_empty())
}

/// Authenticate the request and resolve `(org backend state, org_id,
/// user_id)`. The bearer token is accepted as the static device token
/// (`TASK_WATCH_TOKEN` → deterministic local owner) or validated as a real
/// architect-auth session token (→ that session's user id).
async fn authenticate(
    state: &AppState,
    slug: &str,
    headers: &HeaderMap,
) -> Result<(crate::OrgAppState, Uuid, Uuid), Response> {
    let token = bearer(headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "missing bearer token").into_response())?;
    let org = state.org(slug).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("org `{slug}` not hosted")).into_response()
    })?;
    let org_id = state
        .data_root
        .org(slug)
        .manifest()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("org `{slug}` manifest: {e}"),
            )
                .into_response()
        })?
        .id;

    // Static device token (headless/testing) → the deterministic local owner.
    let device_token = std::env::var("TASK_WATCH_TOKEN").unwrap_or_default();
    if !device_token.is_empty() && token == device_token {
        return Ok((org, org_id, owner_id(org_id)));
    }

    // Otherwise validate as a real session token against this org's auth engine.
    match org
        .auth
        .auth
        .current_session(architect_auth::CurrentSession { token })
        .await
    {
        Ok(bundle) => Ok((org, org_id, bundle.user.id)),
        Err(_) => Err((StatusCode::UNAUTHORIZED, "invalid session token").into_response()),
    }
}

// ── Wire DTOs (the watch's JSON contract) ────────────────────────────────

#[derive(Deserialize, Default)]
struct StartBody {
    /// What's being worked on — free text (dictated on the watch). Optional.
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct CaptureBody {
    /// The fleeting note body (dictated on the watch).
    body: String,
}

/// The subset of a `WorkSession` the watch renders.
#[derive(Serialize)]
struct TimerDto {
    id: String,
    description: String,
    /// RFC-3339 start instant — the watch computes elapsed from this.
    start_time: String,
    /// Present once stopped.
    end_time: Option<String>,
    running: bool,
}

impl From<timer_proto::WorkSession> for TimerDto {
    fn from(w: timer_proto::WorkSession) -> Self {
        TimerDto {
            id: w.id.to_string(),
            description: w.description,
            start_time: w.start_time.to_rfc3339(),
            end_time: w.end_time.map(|t| t.to_rfc3339()),
            running: w.end_time.is_none(),
        }
    }
}

#[derive(Serialize)]
struct CaptureDto {
    id: String,
    created: String,
}

// ── Handlers ─────────────────────────────────────────────────────────────

async fn timer_start(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    body: Option<Json<StartBody>>,
) -> Response {
    let (org, org_id, user_id) = match authenticate(&state, &slug, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let description = body.map(|Json(b)| b.description).unwrap_or_default();
    let req = StartTimerRequest {
        user_id,
        org_id,
        project_id: None,
        project_path: String::new(),
        task_note_path: String::new(),
        description,
    };
    match org.timer.start_timer(req).await {
        Ok(ws) => Json(TimerDto::from(ws)).into_response(),
        Err(e) => (StatusCode::CONFLICT, e.to_string()).into_response(),
    }
}

async fn timer_stop(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (org, _org_id, user_id) = match authenticate(&state, &slug, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match org.timer.stop_timer(user_id).await {
        Ok(ws) => Json(TimerDto::from(ws)).into_response(),
        // No active timer is a normal, non-error outcome for the watch.
        Err(_) => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn timer_active(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (org, _org_id, user_id) = match authenticate(&state, &slug, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match org.timer.active_timer(user_id).await {
        Ok(Some(ws)) => Json(Some(TimerDto::from(ws))).into_response(),
        Ok(None) => Json(None::<TimerDto>).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn inbox_capture(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CaptureBody>,
) -> Response {
    let (org, _org_id, _user_id) = match authenticate(&state, &slug, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let text = body.body.trim();
    if text.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty note").into_response();
    }
    let id = Uuid::new_v4().to_string();
    let created = chrono::Utc::now().to_rfc3339();
    let item = InboxItem::capture(id.clone(), text, "watch", created.clone());
    match org.inbox.upsert_inbox_item(&item) {
        Ok(()) => (StatusCode::CREATED, Json(CaptureDto { id, created })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// The watch bridge routes, to be `.merge`d into the top-level [`crate::router`]
/// (whose trailing `.with_state(state)` supplies the [`AppState`]).
pub fn watch_router() -> Router<AppState> {
    Router::new()
        .route("/org/{slug}/watch/v1/timer/start", post(timer_start))
        .route("/org/{slug}/watch/v1/timer/stop", post(timer_stop))
        .route("/org/{slug}/watch/v1/timer/active", get(timer_active))
        .route("/org/{slug}/watch/v1/inbox", post(inbox_capture))
}
