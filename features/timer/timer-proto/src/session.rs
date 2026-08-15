//! `WorkSession` — one tracked period of work.
//!
//! Disambiguated from `task::TimeEntry` (the inline list
//! that lives in a task note's frontmatter):
//!
//! - `task::TimeEntry` — quick, personal, lives in
//!   markdown, no service / billable / cross-org semantics.
//! - `WorkSession` — DB-backed, user/org-scoped, has
//!   billable flag, currency, optional project link, tags,
//!   and atomic active-timer semantics.
//!
//! References projects by the **frontmatter `id:` UUID**
//! from `features/project/ProjectInfo::id`, not the file
//! path — renaming a project's markdown file doesn't orphan
//! historical sessions. `project_path` is a redundant
//! cache so reports can render quickly without re-scanning
//! the vault.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[architect(table_name = "timer_work_sessions", repo)]
pub struct WorkSession {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Owning organization (`architect_auth::AuthOrganization::id`).
    #[architect(filterable)]
    pub org_id: Uuid,

    /// Who tracked this work (`architect_auth::AuthUser::id`).
    #[architect(filterable, sortable)]
    pub user_id: Uuid,

    /// Project this session is logged against — the
    /// frontmatter `id:` of the project note. `None` =
    /// un-categorized; the session still counts toward the
    /// user's hours but doesn't show in project reports.
    #[architect(filterable)]
    pub project_id: Option<Uuid>,

    /// Vault-relative path of the project note, cached at
    /// start time. Reports use it to render the project
    /// title without an extra lookup; the project's
    /// frontmatter `id:` is the durable reference.
    pub project_path: String,

    /// Free-text description of what was worked on.
    #[architect(fulltext)]
    pub description: String,

    /// Session start. Always populated, including for an
    /// open timer (set to wall-clock when `start` was
    /// called).
    #[architect(filterable, sortable)]
    pub start_time: DateTime<Utc>,

    /// Session end. `None` = currently running. Exactly one
    /// row per `user_id` may have `end_time IS NULL` at a
    /// time; the `timer` crate enforces this via a partial
    /// unique index plus atomic-update semantics in
    /// `start_timer`.
    #[architect(filterable, sortable)]
    pub end_time: Option<DateTime<Utc>>,

    /// Billable flag snapshot. Inherited from the project's
    /// `billableDefault` on start; editable while the
    /// session is open. Snapshot (not derived) so changing
    /// the project default later doesn't retroactively bill
    /// old work.
    #[architect(filterable, sortable)]
    pub billable: bool,

    /// Hourly rate snapshot at close, in cents. Pulled from
    /// the rate cascade (see [`crate::rate`]) when the
    /// session closes; `0` for non-billable or no rate.
    pub rate_cents: i64,

    /// ISO 4217. Mirrors the project's `currency`. Empty for
    /// non-billable sessions with no project link.
    pub currency: String,

    /// Vault-relative path of the task note this session was
    /// tracked against, if any. Reports use it to roll up
    /// time per task note.
    #[architect(filterable)]
    pub task_note_path: String,

    /// The invoice this session was billed on, if any. `None` =
    /// not yet invoiced (the finance backend sets this when an
    /// invoice is generated, and clears it when one is voided /
    /// deleted). Drives the "uninvoiced billable time" view.
    #[architect(filterable)]
    pub invoice_id: Option<Uuid>,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

/// Server-side filter for [`WorkSession`] reads.
#[derive(::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[repr(C)]
pub struct WorkSessionFilter {
    /// Only sessions for this user. `None` = all users.
    pub user_id: Option<Uuid>,
    /// Only sessions on this project. `None` = all.
    pub project_id: Option<Uuid>,
    /// Inclusive lower bound on `start_time`.
    pub since: Option<DateTime<Utc>>,
    /// Exclusive upper bound on `start_time`.
    pub until: Option<DateTime<Utc>>,
    /// `Some(true)` = billable only; `Some(false)` =
    /// non-billable only; `None` = both.
    pub billable: Option<bool>,
    /// `Some(true)` = open (running) timers only;
    /// `Some(false)` = closed only; `None` = both.
    pub open: Option<bool>,
}
