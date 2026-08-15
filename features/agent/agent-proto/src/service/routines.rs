//! Routines — scheduled agent runs.
//!
//! A routine is a prompt the backend runs on a schedule without
//! anyone in the chair: "every morning at 8, summarize what's due
//! today and drop it in my inbox". Hermes gateways serve these as
//! cron jobs (`/api/jobs`); backends without a scheduler report an
//! empty list.
//!
//! Deliberately smaller than the gateway's own job model. Per-job
//! provider/base-url overrides and script hooks stay gateway-side
//! config — the surface here is what a user schedules and watches.

use crate::error::AgentError;
use facet::Facet;

/// One scheduled routine, as the backend reports it.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct Routine {
    /// Backend that owns it (`"hermes"`).
    pub backend_id: String,
    pub id: String,
    /// Friendly name; backends fall back to a prompt prefix.
    pub name: String,
    /// The prompt the agent runs each time.
    pub prompt: String,
    /// Human-readable schedule — `"every 30m"`, `"0 9 * * *"`,
    /// `"once at 2026-08-01 09:00"`.
    pub schedule: String,
    /// `"interval"` | `"cron"` | `"once"`; empty when unreported.
    pub kind: String,
    pub enabled: bool,
    /// Backend lifecycle state (`"scheduled"`, `"paused"`,
    /// `"running"`, `"done"`).
    pub state: String,
    /// RFC-3339; empty when nothing is scheduled (paused, exhausted).
    pub next_run_at: String,
    /// RFC-3339 of the last run; empty if it has never run.
    pub last_run_at: String,
    /// Outcome of the last run (`"ok"`, `"error"`); empty if never run.
    pub last_status: String,
    /// Failure text from the last run; empty when it succeeded.
    pub last_error: String,
    /// Where output goes (`"local"`, `"origin"`, a platform name).
    pub deliver: String,
    /// Runs completed so far.
    pub runs_completed: u32,
    /// Total runs requested; `0` = runs forever.
    pub runs_total: u32,
    /// Skills loaded before the prompt.
    pub skills: Vec<String>,
    /// Per-routine model override; empty = the backend default.
    pub model: String,
}

/// What a caller supplies to schedule a routine.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct NewRoutine {
    /// Backend to schedule on; empty = the default agent backend.
    pub backend_id: String,
    /// Friendly name; empty lets the backend derive one.
    pub name: String,
    /// The prompt to run.
    pub prompt: String,
    /// Schedule expression the backend parses: a duration (`"30m"`,
    /// one-shot), an interval (`"every 2h"`), a cron expression
    /// (`"0 9 * * *"`), or an ISO timestamp (one-shot).
    pub schedule: String,
    /// Delivery target; empty = the backend default (`"local"`).
    pub deliver: String,
    /// Skills to load before the prompt runs.
    pub skills: Vec<String>,
    /// How many times to run; `0` = forever.
    pub repeat: u32,
}

#[architect::rpc]
pub trait Routines {
    /// Every routine across backends (or one, when `backend_id` is
    /// non-empty). Disabled routines are hidden unless asked for.
    fn list_routines(
        &self,
        backend_id: &str,
        include_disabled: bool,
    ) -> Result<Vec<Routine>, AgentError>;

    /// Schedule a new routine.
    fn create_routine(&self, routine: NewRoutine) -> Result<Routine, AgentError>;

    /// Pause or resume. A paused routine keeps its definition but
    /// stops firing, and reports an empty `next_run_at`.
    fn set_routine_paused(
        &self,
        backend_id: &str,
        id: &str,
        paused: bool,
    ) -> Result<Routine, AgentError>;

    /// Run it now, out of band. Doesn't disturb the schedule.
    fn run_routine(&self, backend_id: &str, id: &str) -> Result<Routine, AgentError>;

    /// Remove it entirely.
    fn delete_routine(&self, backend_id: &str, id: &str) -> Result<(), AgentError>;
}
