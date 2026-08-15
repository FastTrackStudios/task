//! Cal.com-style event-type CRUD. The event-type editor binds to
//! this trait; the public booking page also needs it (to fetch
//! the event's metadata before showing the slot picker).

use crate::error::SchedulingError;
use crate::event_type::{EventType, EventTypeId};

#[architect::rpc]
pub trait EventTypes {
    fn list_event_types(&self) -> Result<Vec<EventType>, SchedulingError>;
    fn get_event_type(&self, id: &EventTypeId) -> Result<EventType, SchedulingError>;
    fn upsert_event_type(&self, event_type: &EventType) -> Result<(), SchedulingError>;
    fn delete_event_type(&self, id: &EventTypeId) -> Result<(), SchedulingError>;
}
