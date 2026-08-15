//! Calendar event CRUD — persistence for the schedule's real events.

use crate::cal_event::CalEvent;
use crate::error::SchedulingError;

#[architect::rpc]
pub trait CalendarEvents {
    fn list_events(&self) -> Result<Vec<CalEvent>, SchedulingError>;
    fn upsert_event(&self, event: &CalEvent) -> Result<(), SchedulingError>;
    fn delete_event(&self, id: &str) -> Result<(), SchedulingError>;
}
