//! File-backed [`WorkoutsService`] impl.
//!
//! CRUD for both page types is [`vault_entity::VaultEntityStore`] —
//! one instance per entity, sharing a single vault. Only `log_set` and
//! `start_from_routine`, the parts specific to workouts, live here.

use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use uuid::Uuid;
use vault::Vault;
use vault_entity::VaultEntityStore;

use crate::entity::{Routines, Sessions};
use crate::model::{LoggedSet, Routine, WorkoutSession};
use crate::service::{WorkoutsError, WorkoutsService};
use crate::write::default_session_path;

vault_entity::entity_error_bridge!(WorkoutsError);

#[derive(Clone, architect::HasDispatcher)]
pub struct Store {
    routines: VaultEntityStore<Routines>,
    sessions: VaultEntityStore<Sessions>,
}

impl Store {
    pub fn new(vault: Vault) -> Self {
        Self::from_shared(Arc::new(Mutex::new(vault)))
    }

    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        Self {
            routines: VaultEntityStore::from_shared(inner.clone()),
            sessions: VaultEntityStore::from_shared(inner),
        }
    }

    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.routines.shared()
    }
}

impl WorkoutsService for Store {
    // ── Routines ────────────────────────────────────────
    fn list_routines(&self) -> Result<Vec<Routine>, WorkoutsError> {
        Ok(self.routines.list())
    }

    fn get_routine(&self, id: &str) -> Result<Routine, WorkoutsError> {
        self.routines.get(id).map_err(from_entity_error)
    }

    fn create_routine(&self, routine: Routine) -> Result<Routine, WorkoutsError> {
        self.routines.create(routine).map_err(from_entity_error)
    }

    fn update_routine(&self, routine: Routine) -> Result<Routine, WorkoutsError> {
        self.routines.update(routine).map_err(from_entity_error)
    }

    fn delete_routine(&self, id: &str) -> Result<(), WorkoutsError> {
        self.routines.delete(id).map_err(from_entity_error)
    }

    // ── Sessions ────────────────────────────────────────
    fn list_sessions(&self) -> Result<Vec<WorkoutSession>, WorkoutsError> {
        Ok(self.sessions.list())
    }

    fn get_session(&self, id: &str) -> Result<WorkoutSession, WorkoutsError> {
        self.sessions.get(id).map_err(from_entity_error)
    }

    fn create_session(&self, mut session: WorkoutSession) -> Result<WorkoutSession, WorkoutsError> {
        // The filename carries the session date as a prefix, so the
        // path is resolved here rather than by the shared store.
        if session.path.is_empty() {
            session.path = default_session_path(session.date, &session.name, None);
        }
        self.sessions.create(session).map_err(from_entity_error)
    }

    fn update_session(&self, session: WorkoutSession) -> Result<WorkoutSession, WorkoutsError> {
        self.sessions.update(session).map_err(from_entity_error)
    }

    fn delete_session(&self, id: &str) -> Result<(), WorkoutsError> {
        self.sessions.delete(id).map_err(from_entity_error)
    }

    fn log_set(
        &self,
        session_id: &str,
        mut set: LoggedSet,
    ) -> Result<WorkoutSession, WorkoutsError> {
        let mut session = self.get_session(session_id)?;
        if set.id.is_nil() {
            set.id = Uuid::new_v4();
        }
        if set.order == 0 {
            // 0 sentinel: place at the end. Caller can
            // pass explicit non-zero `order` to insert
            // mid-session.
            set.order = session
                .logged_sets
                .iter()
                .map(|s| s.order)
                .max()
                .map(|m| m + 1)
                .unwrap_or(0);
        }
        session.logged_sets.push(set);
        self.update_session(session)
    }

    fn start_from_routine(
        &self,
        routine_id: &str,
        day_name: &str,
        date: &str,
    ) -> Result<WorkoutSession, WorkoutsError> {
        let routine = self.get_routine(routine_id)?;
        let day = routine
            .days
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(day_name))
            .ok_or_else(|| WorkoutsError::NotFound(format!("day {day_name} in routine")))?;
        let date: NaiveDate = date
            .parse()
            .map_err(|e| WorkoutsError::BadRequest(format!("date: {e}")))?;

        // Expand each slot's programmed `sets` count into
        // empty LoggedSet rows so the UI can step through
        // and fill actuals.
        let mut logged_sets = Vec::new();
        let mut order: u32 = 0;
        for slot in &day.slots {
            let reps_hint = slot
                .reps
                .as_deref()
                .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let count = slot.sets.unwrap_or(1).max(1);
            for _ in 0..count {
                logged_sets.push(LoggedSet {
                    id: Uuid::new_v4(),
                    exercise_id: slot.exercise_id,
                    exercise_name: slot.exercise_name.clone(),
                    order,
                    reps: reps_hint,
                    weight_kg: slot.weight_kg.unwrap_or(0.0),
                    rir: slot.rir,
                    rpe: None,
                    completed: false,
                    note: slot.note.clone(),
                });
                order += 1;
            }
        }

        let session = WorkoutSession {
            path: String::new(),
            id: Uuid::nil(),
            name: format!("{} — {}", routine.name, day_name),
            date,
            routine_id: Some(routine.id),
            day_name: Some(day_name.to_string()),
            logged_sets: crate::model::LoggedSets(logged_sets),
            status: crate::model::SessionStatus::Planned.as_str().to_string(),
            duration_minutes: None,
            tags: crate::model::Tags::default(),
            date_created: None,
            date_modified: None,
            details: String::new(),
        };
        self.create_session(session)
    }
}
