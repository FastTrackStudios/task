//! Walk the vault for routines + workout sessions.

use chrono::NaiveDate;
use uuid::Uuid;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::{Routines, Sessions};
use crate::model::{Routine, WorkoutSession};

/// Every routine page, parse failures logged and skipped.
pub fn scan_routines(vault: &Vault) -> Vec<Routine> {
    VaultEntityStore::<Routines>::scan(vault)
}

/// Every workout-session page, parse failures logged and skipped.
pub fn scan_sessions(vault: &Vault) -> Vec<WorkoutSession> {
    VaultEntityStore::<Sessions>::scan(vault)
}

/// Sessions in `[start, end)`. Useful for weekly volume
/// summaries.
pub fn sessions_between(vault: &Vault, start: NaiveDate, end: NaiveDate) -> Vec<WorkoutSession> {
    scan_sessions(vault)
        .into_iter()
        .filter(|s| s.date >= start && s.date < end)
        .collect()
}

/// Sessions that logged at least one set of `exercise_id`.
/// Drives the "show me bench-press progression" view —
/// caller charts the resulting [`crate::model::LoggedSet`]
/// max weights over time.
pub fn sessions_for_exercise(vault: &Vault, exercise_id: Uuid) -> Vec<WorkoutSession> {
    scan_sessions(vault)
        .into_iter()
        .filter(|s| {
            s.logged_sets
                .iter()
                .any(|set| set.exercise_id == exercise_id)
        })
        .collect()
}
