//! Availability-schedule CRUD. The schedule editor binds to this
//! trait; slot generation (`Slots`) typically reads schedules via
//! its own backend internals rather than going through this trait.

use crate::error::SchedulingError;
use crate::schedule::{AvailabilitySchedule, ScheduleId};

#[architect::rpc]
pub trait Schedules {
    fn list_schedules(&self) -> Result<Vec<AvailabilitySchedule>, SchedulingError>;
    fn get_schedule(&self, id: &ScheduleId) -> Result<AvailabilitySchedule, SchedulingError>;
    fn upsert_schedule(&self, schedule: &AvailabilitySchedule) -> Result<(), SchedulingError>;
    fn delete_schedule(&self, id: &ScheduleId) -> Result<(), SchedulingError>;
}
