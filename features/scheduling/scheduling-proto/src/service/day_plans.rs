//! Per-day plan CRUD — the day-by-day schedule editor's surface.

use crate::day_plan::DayPlan;
use crate::error::SchedulingError;

#[architect::rpc]
pub trait DayPlans {
    /// The saved plan for `date` (ISO `YYYY-MM-DD`), or `None` when
    /// the date hasn't been edited yet (the caller materializes one
    /// from the matching template).
    fn get_day_plan(&self, date: &str) -> Result<Option<DayPlan>, SchedulingError>;
    /// Save (replacing) the plan for `plan.date`.
    fn upsert_day_plan(&self, plan: &DayPlan) -> Result<(), SchedulingError>;
    /// Drop a date's saved plan, reverting it to the template.
    fn delete_day_plan(&self, date: &str) -> Result<(), SchedulingError>;
}
