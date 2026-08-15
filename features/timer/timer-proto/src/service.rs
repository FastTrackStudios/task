//! Slim domain trait for atomic / lifecycle-aware ops.
//!
//! CRUD on the entities (Client, WorkSession, Tag, Rate
//! tables) is the architect-emitted per-entity Repo trait.
//! This trait carries the operations CRUD can't enforce:
//!
//! - **`start_timer`** atomically asserts the "one open
//!   session per user" invariant.
//! - **`stop_timer`** closes the currently-open session
//!   for a user, snapshots the rate cascade into
//!   `rate_cents` + `currency`, and emits the closed row.
//! - **`active_timer`** is the cheap "is anything running
//!   right now?" lookup the UI polls.
//! - **`switch_timer`** is a compound that stops the open
//!   session (if any) and starts a new one in one
//!   transaction — the common case for a user moving from
//!   task to task.
//! - **`resolve_rate`** runs the cascade and returns the
//!   rate cents + currency a session would close at, for
//!   the UI to render before stop.
//!
//! Owned-String params throughout — same convention as
//! `agent-proto`'s service traits, dictated by what
//! `#[architect::rpc]`'s async dispatcher emits cleanly.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::TimerError;
use crate::rate::{OrgMemberRate, ProjectMemberRate};
use crate::session::{WorkSession, WorkSessionFilter};

/// One work-session change, broadcast to every [`TimerService`]
/// subscriber on each successful session mutation.
///
/// ## What the stream carries
///
/// **Sessions only.** The "is a timer running right now?" state is
/// *derived*: a running timer is exactly a session row with
/// `end_time = None`, so streaming full post-write [`WorkSession`]
/// rows covers the visible live surface — a timer starting
/// (`Upserted` with open `end_time`), stopping (`Upserted` with the
/// closed row, authoritative rate snapshot included), switching (two
/// `Upserted`s: the closed row, then the new open one), retro-logs,
/// edits, and deletes. Rate-table changes (`set_org_member_rate` /
/// `set_project_member_rate`) do NOT stream — rates are a settings
/// surface read on demand, and future sessions carry their snapshot.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// Changes only — no `Snapshot` variant. Fetch the session list once
/// via [`TimerService::list_sessions`] (after subscribing, so
/// nothing is missed in between), then fold:
///
/// - [`TimerEvent::Upserted`] carries the **full post-write**
///   [`WorkSession`] — replace (or insert) the row with a matching
///   `id`. Idempotent re-application is harmless.
/// - [`TimerEvent::Deleted`] — remove the row with that `id`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ::facet::Facet)]
#[repr(u8)]
// Upserted carries the full WorkSession by design (idempotent
// full-state payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum TimerEvent {
    /// A session was started, stopped, logged, or edited — the
    /// payload is the complete state after the write.
    Upserted(WorkSession),
    /// The session with this id was permanently removed.
    Deleted(Uuid),
}

#[architect::rpc]
pub trait TimerService {
    /// Start a new work session for the calling user. Fails
    /// with [`TimerError::AlreadyRunning`] if the user
    /// already has an open session. The new row's
    /// `billable` is seeded from the project's
    /// `billableDefault` (or `false` when no project link).
    async fn start_timer(&self, req: StartTimerRequest) -> Result<WorkSession, TimerError>;

    /// Stop the calling user's open session. Snapshots
    /// `rate_cents` + `currency` via the rate cascade, sets
    /// `end_time = now`, returns the closed row. Fails with
    /// [`TimerError::NoActiveTimer`] when nothing is open.
    async fn stop_timer(&self, user_id: Uuid) -> Result<WorkSession, TimerError>;

    /// Read the calling user's currently-open session.
    /// Returns `Ok(None)` (not an error) when nothing is
    /// running — this is the cheap UI poll.
    async fn active_timer(&self, user_id: Uuid) -> Result<Option<WorkSession>, TimerError>;

    /// Stop-then-start in one transaction. Equivalent to
    /// `stop_timer` followed by `start_timer`, but atomic so
    /// a UI button-press doesn't briefly show two open
    /// sessions (or none) under contention. Returns
    /// `(closed_or_none, started)`.
    async fn switch_timer(
        &self,
        req: StartTimerRequest,
    ) -> Result<(Option<WorkSession>, WorkSession), TimerError>;

    /// Manually retro-log a session (start + end in the
    /// past). Skips the active-timer invariant; the open
    /// session stays open. The frontend uses this for
    /// "I forgot to start the timer" entries.
    async fn log_session(&self, req: LogSessionRequest) -> Result<WorkSession, TimerError>;

    /// Resolve the rate cascade for the given
    /// `(user_id, project_id)`. Returns the cents/hour and
    /// the ISO 4217 currency that would be snapshotted if a
    /// session closed right now. Cents = 0 when no rate is
    /// configured at any level.
    async fn resolve_rate(
        &self,
        user_id: Uuid,
        project_id: Option<Uuid>,
    ) -> Result<RateResolution, TimerError>;

    /// List sessions matching `filter` (by user / project / date range
    /// / billable / open). Powers the timer page's history + totals.
    async fn list_sessions(
        &self,
        filter: WorkSessionFilter,
    ) -> Result<Vec<WorkSession>, TimerError>;

    /// Edit an existing session. Only the `Some(_)` fields in `req` are
    /// applied; the rest are left as-is. After applying, the rate is
    /// re-snapshotted from the cascade against the (possibly changed)
    /// user / project / billable so the stored `rate_cents` stays
    /// consistent — editing a session re-resolves its rate. Returns the
    /// updated row. Fails with [`TimerError::NotFound`] for an unknown id.
    async fn update_session(&self, req: UpdateSessionRequest) -> Result<WorkSession, TimerError>;

    /// Permanently delete a session by id. Idempotent — deleting an
    /// already-gone id is `Ok(())`.
    async fn delete_session(&self, id: Uuid) -> Result<(), TimerError>;

    /// Upsert the **org-level** hourly rate (cents) for a member
    /// (cascade level 3). Returns the stored row. `currency` is ISO
    /// 4217. New sessions logged for that member snapshot this rate.
    async fn set_org_member_rate(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        hourly_cents: i64,
        currency: String,
    ) -> Result<OrgMemberRate, TimerError>;

    /// Upsert the **per-project** hourly rate (cents) for a member
    /// (cascade level 1). This is what lets several members bill a
    /// single project at *different* rates; currency inherits the
    /// project. Returns the stored row.
    async fn set_project_member_rate(
        &self,
        project_id: Uuid,
        user_id: Uuid,
        hourly_cents: i64,
    ) -> Result<ProjectMemberRate, TimerError>;

    /// Every org-level member rate configured for `org_id`.
    async fn list_org_member_rates(&self, org_id: Uuid) -> Result<Vec<OrgMemberRate>, TimerError>;

    /// Every per-member rate set on `project_id`.
    async fn list_project_member_rates(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<ProjectMemberRate>, TimerError>;

    /// Every [`crate::Tag`] in `org_id`.
    async fn list_tags(&self, org_id: Uuid) -> Result<Vec<crate::Tag>, TimerError>;

    /// Idempotent tag upsert by `(org_id, name)` — returns the
    /// existing row (its stored color untouched) or the freshly
    /// inserted one.
    async fn create_tag(
        &self,
        org_id: Uuid,
        name: String,
        color: String,
    ) -> Result<crate::Tag, TimerError>;

    /// Delete a tag by name; its session joins go with it (FK
    /// cascade). Returns the deleted row. Fails with
    /// [`TimerError::TagNotFound`] for an unknown name.
    async fn delete_tag(&self, org_id: Uuid, name: String) -> Result<crate::Tag, TimerError>;

    /// Ensure each tag in `names` exists in `org_id` (auto-create,
    /// empty color) and attach it to `session_id`. Already-attached
    /// pairs are skipped.
    async fn attach_tags(
        &self,
        session_id: Uuid,
        org_id: Uuid,
        names: Vec<String>,
    ) -> Result<(), TimerError>;

    /// Detach tags from `session_id`. `all = true` removes every
    /// tag (`names` ignored); otherwise removes the named tags and
    /// returns how many of `names` matched an existing tag in the
    /// org (0 = none of those tags exist).
    async fn detach_tags(
        &self,
        session_id: Uuid,
        org_id: Uuid,
        names: Vec<String>,
        all: bool,
    ) -> Result<u64, TimerError>;

    /// Every work-session change, as it happens — fires on each
    /// successful start / stop / switch / log / update / delete.
    /// See [`TimerEvent`] for the fetch-once-then-fold subscriber
    /// contract (sessions only; rate edits don't stream).
    #[subscribe]
    fn events(&self) -> TimerEvent;
}

/// Args for [`TimerService::update_session`]. `id` selects the row;
/// every other field is `Option` — `Some` overwrites, `None` leaves the
/// current value. `org_id` is intentionally not editable.
#[derive(
    ::facet::Facet, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default,
)]
#[repr(C)]
pub struct UpdateSessionRequest {
    pub id: Uuid,
    /// Reassign to a different member (also re-resolves the rate).
    pub user_id: Option<Uuid>,
    /// Set the project link (re-resolves the rate).
    pub project_id: Option<Uuid>,
    pub project_path: Option<String>,
    pub task_note_path: Option<String>,
    pub description: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub billable: Option<bool>,
    /// Keep the stored `rate_cents` + `currency` instead of
    /// re-snapshotting from the cascade. For historical
    /// corrections (`task timer reassign-user` without
    /// `--rerate`) where already-billed amounts must not
    /// shift. Default `false` = today's re-snapshot
    /// behaviour.
    pub preserve_rate: bool,
}

/// Args for [`TimerService::start_timer`].
#[derive(::facet::Facet, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct StartTimerRequest {
    pub user_id: Uuid,
    pub org_id: Uuid,
    /// Project the session belongs to. `None` = uncategorized
    /// (still tracked, just doesn't roll up).
    pub project_id: Option<Uuid>,
    /// Vault path cache. Empty = no project link.
    pub project_path: String,
    pub task_note_path: String,
    pub description: String,
}

/// Args for [`TimerService::log_session`].
#[derive(::facet::Facet, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct LogSessionRequest {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub project_id: Option<Uuid>,
    pub project_path: String,
    pub task_note_path: String,
    pub description: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    /// If `Some(false)`, log as non-billable regardless of
    /// project default. `None` inherits.
    pub billable_override: Option<bool>,
}

/// Result of [`TimerService::resolve_rate`].
#[derive(::facet::Facet, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RateResolution {
    pub hourly_cents: i64,
    pub currency: String,
    /// Which level produced the rate. Useful for the UI
    /// to render "Using project member rate" / "Using org
    /// default" indicators.
    pub source: RateSource,
}

/// Where the resolved rate came from. Mirrors the cascade
/// in [`crate::rate`].
#[derive(
    ::facet::Facet, serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq,
)]
#[repr(u8)]
pub enum RateSource {
    /// No billable rate at any level. Session goes
    /// non-billable.
    None = 0,
    /// `ProjectMemberRate` row matched.
    ProjectMember = 1,
    /// Project markdown's `defaultRateCents` field.
    ProjectDefault = 2,
    /// `OrgMemberRate` row matched.
    OrgMember = 3,
}
