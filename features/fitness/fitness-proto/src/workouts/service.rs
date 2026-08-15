//! `WorkoutsService` — wire surface for both routines
//! and sessions. Single trait keeps the surface coherent
//! since the two page types compose tightly (a session
//! references a routine, set logs reference an exercise).

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::model::{LoggedSet, Routine, WorkoutSession};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum WorkoutsError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait WorkoutsService {
    // ── Routines ────────────────────────────────────────
    fn list_routines(&self) -> Result<Vec<Routine>, WorkoutsError>;

    fn get_routine(&self, id: &str) -> Result<Routine, WorkoutsError>;

    fn create_routine(&self, routine: Routine) -> Result<Routine, WorkoutsError>;

    fn update_routine(&self, routine: Routine) -> Result<Routine, WorkoutsError>;

    fn delete_routine(&self, id: &str) -> Result<(), WorkoutsError>;

    // ── Sessions ────────────────────────────────────────
    fn list_sessions(&self) -> Result<Vec<WorkoutSession>, WorkoutsError>;

    fn get_session(&self, id: &str) -> Result<WorkoutSession, WorkoutsError>;

    fn create_session(&self, session: WorkoutSession) -> Result<WorkoutSession, WorkoutsError>;

    fn update_session(&self, session: WorkoutSession) -> Result<WorkoutSession, WorkoutsError>;

    fn delete_session(&self, id: &str) -> Result<(), WorkoutsError>;

    /// Append a single set to a session — convenience
    /// for "I just finished a set, log it" UIs. Assigns
    /// `set.id` if nil and `set.order` if `0` (places at
    /// the end). Returns the updated session.
    fn log_set(&self, session_id: &str, set: LoggedSet) -> Result<WorkoutSession, WorkoutsError>;

    /// Start a fresh session from a routine + day. Pulls
    /// programmed slots into a `planned` session shell so
    /// the lifter can step through and log actuals.
    fn start_from_routine(
        &self,
        routine_id: &str,
        day_name: &str,
        date: &str,
    ) -> Result<WorkoutSession, WorkoutsError>;
}
