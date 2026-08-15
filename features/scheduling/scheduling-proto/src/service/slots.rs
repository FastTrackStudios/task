//! Open-slot listing — the derived view the public booking page
//! consumes. Split into its own trait because some backends only
//! support reads (e.g. a future read-only federation peer that
//! exposes a host's calendar without letting external callers
//! mutate it).

use crate::error::SchedulingError;
use crate::schedule::{SlotQuery, TimeSlot};

#[architect::rpc]
pub trait Slots {
    /// Open slots for the event type inside `query.from_utc ..
    /// query.to_utc`. The backend intersects the event type's
    /// schedule rules with existing bookings.
    fn list_open_slots(&self, query: &SlotQuery) -> Result<Vec<TimeSlot>, SchedulingError>;
}
