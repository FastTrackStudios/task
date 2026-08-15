//! Unified fitness store — wraps the four sub-crate
//! stores around one shared `vault::Vault` mutex so reads
//! from any surface see the same snapshot.

use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use vault::Vault;

use crate::summary::{DailySummary, compute_daily_summary};

#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Vault>>,
    pub exercises: exercises::Store,
    pub workouts: workouts::Store,
    pub body: body::Store,
    pub intake: intake::Store,
}

impl Store {
    /// Build all four sub-stores around the same vault.
    pub fn new(vault: Vault) -> Self {
        let body = body::Store::new(vault);
        let inner = body.shared();
        let exercises = exercises::Store::from_shared(inner.clone());
        let workouts = workouts::Store::from_shared(inner.clone());
        let intake = intake::Store::from_shared(inner.clone());
        Self {
            inner,
            exercises,
            workouts,
            body,
            intake,
        }
    }

    /// Reuse a vault mutex already owned by another
    /// feature (mealplan, etc.). Pairs with that
    /// feature's `Store::shared`.
    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        let exercises = exercises::Store::from_shared(inner.clone());
        let workouts = workouts::Store::from_shared(inner.clone());
        let body = body::Store::from_shared(inner.clone());
        let intake = intake::Store::from_shared(inner.clone());
        Self {
            inner,
            exercises,
            workouts,
            body,
            intake,
        }
    }

    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.clone()
    }

    /// Compose the day's intake + sessions + latest weight
    /// into a [`DailySummary`]. Convenience that drives the
    /// "today" surface in CLI/UI clients.
    pub fn daily_summary(&self, date: NaiveDate) -> DailySummary {
        let intake_log = intake::for_day(&self.inner.lock().expect("fitness store poisoned"), date);
        let sessions = workouts::scan_sessions(&self.inner.lock().expect("fitness store poisoned"));
        let weight_metric = body::by_kind(
            &self.inner.lock().expect("fitness store poisoned"),
            "weight",
        );
        compute_daily_summary(date, intake_log.as_ref(), &sessions, weight_metric.as_ref())
    }
}
