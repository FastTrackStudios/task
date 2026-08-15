//! `DailySummary` — the unified "how was today" view.
//!
//! Pulls together what's tracked across the four fitness
//! sub-crates: latest body weight, daily intake totals,
//! and any workout sessions on the date. Pure function;
//! consumers (CLI, UI, agents) drive the underlying
//! stores then call [`compute_daily_summary`].

use chrono::NaiveDate;
use facet::Facet;
use mealplan::cookbook::Nutrition;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct DailySummary {
    pub date: NaiveDate,

    /// Total nutrition consumed across the day's intake
    /// log entries. `None` when no entries had nutrition.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "intakeTotal"
    )]
    pub intake_total: Option<Nutrition>,

    /// Caller-supplied daily targets (typically read off
    /// `IntakeLog::target`). Same shape as `intake_total`
    /// so the UI can render progress bars directly.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<Nutrition>,

    /// Total volume (kg-reps) across every session on
    /// this date. `0.0` when nothing was lifted.
    #[serde(rename = "workoutVolume")]
    pub workout_volume: f64,

    /// Total reps across every session on this date.
    #[serde(rename = "workoutReps")]
    pub workout_reps: u32,

    /// How many sessions happened on this date. Usually
    /// 0 or 1; 2+ on doubles days.
    #[serde(rename = "sessionCount")]
    pub session_count: u32,

    /// Latest weight entry as of `date` (across the
    /// weight metric's full log). Drives the "current
    /// weight" surface; `None` until at least one entry
    /// exists.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "latestWeight"
    )]
    pub latest_weight: Option<WeightSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct WeightSnapshot {
    pub date: NaiveDate,
    pub value: f64,
    pub unit: String,
}

/// Compute a [`DailySummary`] for `date` from the day's
/// intake log + sessions + the weight metric's history.
/// All inputs are pre-resolved; this function is pure +
/// I/O-free so it can run anywhere.
pub fn compute_daily_summary(
    date: NaiveDate,
    intake: Option<&intake::IntakeLog>,
    sessions: &[workouts::WorkoutSession],
    weight_metric: Option<&body::BodyMetric>,
) -> DailySummary {
    let (intake_total, target) = intake
        .map(|i| (i.total(), i.target.0.clone()))
        .unwrap_or((None, None));

    let today_sessions: Vec<&workouts::WorkoutSession> =
        sessions.iter().filter(|s| s.date == date).collect();
    let workout_volume = today_sessions.iter().map(|s| s.total_volume()).sum();
    let workout_reps = today_sessions.iter().map(|s| s.total_reps()).sum();
    let session_count = today_sessions.len() as u32;

    let latest_weight = weight_metric.and_then(|m| {
        // Most recent entry on or before `date`.
        m.entries
            .iter()
            .filter(|e| e.date <= date)
            .max_by_key(|e| (e.date, e.id))
            .map(|e| WeightSnapshot {
                date: e.date,
                value: e.value,
                unit: e
                    .unit
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| m.unit.clone()),
            })
    });

    DailySummary {
        date,
        intake_total,
        target,
        workout_volume,
        workout_reps,
        session_count,
        latest_weight,
    }
}
