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
) -> Result<(std::sync::Arc<crate::OrgAppState>, Uuid, Uuid), Response> {
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
    let (org, _org_id, user_id) = match authenticate(&state, &slug, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let text = body.body.trim();
    if text.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty note").into_response();
    }
    let id = Uuid::new_v4().to_string();
    let created = chrono::Utc::now().to_rfc3339();
    let mut item = InboxItem::capture(id.clone(), text, "watch", created.clone());
    // The wearer captured it: the account the device authenticated as.
    let creator = org.creators.resolve(&user_id.to_string()).await;
    crate::attribution::attribute_inbox(&mut item, Some(&creator));
    match org.inbox.upsert_inbox_item(&item) {
        Ok(()) => (StatusCode::CREATED, Json(CaptureDto { id, created })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Suggested: the one thing worth doing now ─────────────────────────────
//
// The same pick as the Tasks page's Suggested slot (`task_proto::suggest`,
// `crates/ui/src/suggested.rs`), computed here from the org's backends so
// the wrist and the phone agree. The watch sends ITS local date and
// minute-of-day: the server doesn't know the wearer's timezone. "Not now"
// lives on the watch (a per-day declined list it sends back as
// `declined`), like the web slot keeps it per device.

/// `GET …/suggested` query.
#[derive(Deserialize)]
struct SuggestedQuery {
    /// The watch's local date, `YYYY-MM-DD`.
    date: String,
    /// The watch's local minute since midnight (0–1439).
    minute: u16,
    /// Comma-separated `key`s set aside with "Not now".
    #[serde(default)]
    declined: String,
}

/// `POST …/suggested/act` body.
#[derive(Deserialize)]
struct ActBody {
    /// The `key` of the suggestion the watch showed.
    key: String,
    date: String,
    minute: u16,
    /// Keys set aside, so the returned `next` skips them too.
    #[serde(default)]
    declined: Vec<String>,
}

/// One suggestion, worded for the watch (`Suggestion::card`).
#[derive(Serialize)]
struct SuggestionDto {
    /// What "Not now" remembers, and what `act` names.
    key: String,
    /// `planned` | `mode` | `continue` | `overdue` | `process` | `today`.
    kind: &'static str,
    /// Why now, small, above the title ("Maintenance hour · until 5:00 PM").
    eyebrow: String,
    title: String,
    /// A quiet line under the title; may be empty.
    detail: String,
    /// Eyebrow and detail in one line.
    why: String,
    /// The primary action: `Start` | `Done` | `Process`.
    action: &'static str,
    /// The task it is about, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    /// For `mode`: the session description Start opens ("Maintenance hour").
    #[serde(skip_serializing_if = "Option::is_none")]
    mode_title: Option<String>,
}

/// `POST …/suggested/act` response: the timer the action opened (Start),
/// if any, and the suggestion that shows now.
#[derive(Serialize)]
struct ActDto {
    timer: Option<TimerDto>,
    next: Option<SuggestionDto>,
}

fn kind_of(s: &task::suggest::Suggestion) -> &'static str {
    use task::suggest::Suggestion as S;
    match s {
        S::StartPlanned { .. } => "planned",
        S::StartMode { .. } => "mode",
        S::Continue(_) => "continue",
        S::Overdue(_) => "overdue",
        S::Process(_) => "process",
        S::DueToday(_) => "today",
    }
}

impl From<&task::suggest::Suggestion> for SuggestionDto {
    fn from(s: &task::suggest::Suggestion) -> Self {
        let card = s.card();
        SuggestionDto {
            key: s.key(),
            kind: kind_of(s),
            eyebrow: card.eyebrow,
            title: card.title,
            detail: card.detail,
            why: card.why,
            action: card.action,
            task_id: s.task().map(|t| t.id.to_string()),
            mode_title: match s {
                task::suggest::Suggestion::StartMode { mode, .. } => Some(mode.title.clone()),
                _ => None,
            },
        }
    }
}

/// Everything the pick reads, gathered from the org's backends.
struct Gathered {
    hint: Option<task::suggest::NowHint>,
    mine: Vec<task::TaskInfo>,
    active: Option<timer_proto::WorkSession>,
    to_process: usize,
    today: String,
}

fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, msg.into()).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}

/// What today's plan says about `minute`: the saved plan for `date`
/// merged with its weekday/weekend template (`scheduling_proto::resolve_day`,
/// the calendar's own resolution), read by `task_proto::suggest::now_hint`.
#[cfg(feature = "plugin-scheduling")]
fn plan_hint(
    org: &crate::OrgAppState,
    date: chrono::NaiveDate,
    minute: u16,
) -> Result<Option<task::suggest::NowHint>, Response> {
    use scheduling_proto::{DayPlans as _, DayTemplates as _};
    let saved = org
        .scheduling
        .get_day_plan(&date.to_string())
        .map_err(internal)?;
    let templates = org.scheduling.list_day_templates().map_err(internal)?;
    let (plan, _) = scheduling_proto::resolve_day(date, saved, &templates);
    Ok(task::suggest::now_hint(&plan.blocks.0, minute))
}

#[cfg(not(feature = "plugin-scheduling"))]
#[allow(clippy::unnecessary_wraps)]
fn plan_hint(
    _org: &crate::OrgAppState,
    _date: chrono::NaiveDate,
    _minute: u16,
) -> Result<Option<task::suggest::NowHint>, Response> {
    Ok(None)
}

async fn gather(
    org: &crate::OrgAppState,
    user_id: Uuid,
    date: &str,
    minute: u16,
) -> Result<Gathered, Response> {
    use task::TaskService as _;
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| bad_request("date must be YYYY-MM-DD"))?;
    if minute >= 24 * 60 {
        return Err(bad_request("minute must be 0..1440"));
    }
    let hint = plan_hint(org, day, minute)?;
    let me = user_id.to_string();
    let mine: Vec<task::TaskInfo> = org
        .tasks
        .list()
        .map_err(internal)?
        .into_iter()
        .filter(|t| task::is_mine(t, Some(&me)))
        .collect();
    let active = org.timer.active_timer(user_id).await.map_err(internal)?;
    let to_process = inbox_proto::due_queue(&org.inbox.list_inbox().map_err(internal)?, date).len();
    Ok(Gathered {
        hint,
        mine,
        active,
        to_process,
        today: day.to_string(),
    })
}

/// Run the pick over what was gathered: every candidate in order when
/// `all` (what `act` matches a key against — "Not now" doesn't make a
/// suggestion stop applying), else just the one to show.
fn pick(g: &Gathered, declined: &[String], all: bool) -> Vec<task::suggest::Suggestion> {
    let refs: Vec<&task::TaskInfo> = g.mine.iter().collect();
    // The task a running timer is on, by its note path.
    let timing = g.active.as_ref().and_then(|s| {
        let path = &s.task_note_path;
        g.mine
            .iter()
            .find(|t| !path.is_empty() && t.path == *path)
            .map(|t| t.id)
    });
    let input = task::suggest::SuggestInput {
        now: g.hint.as_ref(),
        mine: &refs,
        timing,
        to_process: g.to_process,
        today: &g.today,
        declined,
    };
    if all {
        task::suggest::candidates(&input)
    } else {
        task::suggest::suggest(&input).into_iter().collect()
    }
}

async fn suggested(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<SuggestedQuery>,
) -> Response {
    let (org, _org_id, user_id) = match authenticate(&state, &slug, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let g = match gather(&org, user_id, &q.date, q.minute).await {
        Ok(g) => g,
        Err(r) => return r,
    };
    let declined: Vec<String> = q
        .declined
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .collect();
    let s = pick(&g, &declined, false).into_iter().next();
    architect_telemetry::wide::set("watch.suggested", s.as_ref().map_or("none", kind_of));
    Json(s.as_ref().map(SuggestionDto::from)).into_response()
}

/// Set a task's status the way the board's `SetStatus` does: stamp the
/// modification, and the completion day when it finishes.
fn set_status(
    org: &crate::OrgAppState,
    id: Uuid,
    status: &str,
    today: chrono::NaiveDate,
) -> Result<(), Response> {
    use task::TaskService as _;
    let mut t = org.tasks.get(id).map_err(internal)?;
    t.status = status.to_owned();
    t.date_modified = Some(chrono::Utc::now());
    t.completed_date = (status == "done").then_some(today);
    org.tasks.update(t).map_err(internal)?;
    Ok(())
}

/// Perform the suggestion's primary action, if `key` still applies now:
/// Start opens a timer session (on the planned task, which also goes
/// in-progress; or named for the block's mode — that session IS the
/// mode), Done completes the task. Process is the phone's job (422).
/// A key that no longer applies is 409. Returns [`ActDto`].
async fn suggested_act(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ActBody>,
) -> Response {
    use task::suggest::Suggestion as S;
    let (org, org_id, user_id) = match authenticate(&state, &slug, &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let g = match gather(&org, user_id, &body.date, body.minute).await {
        Ok(g) => g,
        Err(r) => return r,
    };
    let Some(s) = pick(&g, &[], true)
        .into_iter()
        .find(|s| s.key() == body.key)
    else {
        architect_telemetry::wide::set("watch.act.outcome", "stale");
        return (StatusCode::CONFLICT, "that suggestion no longer applies").into_response();
    };
    architect_telemetry::wide::set("watch.suggested", kind_of(&s));
    let today = chrono::NaiveDate::parse_from_str(&g.today, "%Y-%m-%d").unwrap_or_default();
    let start = |description: String, task: Option<&task::TaskInfo>| StartTimerRequest {
        user_id,
        org_id,
        project_id: task.and_then(|t| t.project_id),
        project_path: String::new(),
        task_note_path: task.map(|t| t.path.clone()).unwrap_or_default(),
        description,
    };
    let timer = match &s {
        S::StartPlanned { task, .. } => {
            // Whatever was running stops where this starts.
            let started = match org
                .timer
                .switch_timer(start(task.title.clone(), Some(task)))
                .await
            {
                Ok((_, ws)) => ws,
                Err(e) => return (StatusCode::CONFLICT, e.to_string()).into_response(),
            };
            if let Err(r) = set_status(&org, task.id, "in-progress", today) {
                return r;
            }
            Some(TimerDto::from(started))
        }
        S::StartMode { mode, .. } => match org
            .timer
            .switch_timer(start(mode.title.clone(), None))
            .await
        {
            Ok((_, ws)) => Some(TimerDto::from(ws)),
            Err(e) => return (StatusCode::CONFLICT, e.to_string()).into_response(),
        },
        S::Continue(t) | S::Overdue(t) | S::DueToday(t) => {
            if let Err(r) = set_status(&org, t.id, "done", today) {
                return r;
            }
            None
        }
        S::Process(_) => {
            architect_telemetry::wide::set("watch.act.outcome", "refused");
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "process captures on the phone",
            )
                .into_response();
        }
    };
    architect_telemetry::wide::set("watch.act.outcome", "acted");
    // What shows now, after the action.
    let g = match gather(&org, user_id, &body.date, body.minute).await {
        Ok(g) => g,
        Err(r) => return r,
    };
    let next = pick(&g, &body.declined, false)
        .first()
        .map(SuggestionDto::from);
    Json(ActDto { timer, next }).into_response()
}

/// The watch bridge routes, to be `.merge`d into the top-level [`crate::router`]
/// (whose trailing `.with_state(state)` supplies the [`AppState`]).
pub fn watch_router() -> Router<AppState> {
    Router::new()
        .route("/org/{slug}/watch/v1/timer/start", post(timer_start))
        .route("/org/{slug}/watch/v1/timer/stop", post(timer_stop))
        .route("/org/{slug}/watch/v1/timer/active", get(timer_active))
        .route("/org/{slug}/watch/v1/inbox", post(inbox_capture))
        .route("/org/{slug}/watch/v1/suggested", get(suggested))
        .route("/org/{slug}/watch/v1/suggested/act", post(suggested_act))
}
