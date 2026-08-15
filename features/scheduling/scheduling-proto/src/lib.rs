// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the scheduling feature.
//!
//! Two distinct surfaces share this proto:
//!
//! 1. **Personal scheduling** — [`DayTemplate`] / [`TimeBlock`] /
//!    [`BlockCategory`] capture the user's daily rhythm. The
//!    personal day-template editor binds to [`service::DayTemplates`].
//!
//! 2. **Business / cal.com-style scheduling** — [`EventType`] is
//!    the bookable surface backed by an [`AvailabilitySchedule`].
//!    The public booking page binds to [`service::EventTypes`] +
//!    [`service::Slots`] + [`service::Bookings`].
//!
//! ## Capability sub-traits
//!
//! No umbrella trait. Each capability is its own
//! `#[architect::rpc]` so function signatures express exactly
//! what they need:
//!
//! ```ignore
//! fn render_booking_page<S: EventTypes + Slots + Bookings>(s: &S, /* … */) { /* … */ }
//! fn save_template       <S: DayTemplates>                 (s: &S, /* … */) { /* … */ }
//! ```
//!
//! Backends mix + match — a `CalDAV` peer might impl just
//! `EventTypes + Slots`; the local `VaultScheduler` impls all
//! five.
//!
//! Both surfaces live in the same vault as plain markdown files —
//! the `scheduling` crate (sibling) owns the parse / write side.

pub mod booking;
pub mod cal_event;
pub mod day_plan;
pub mod error;
pub mod event_type;
pub mod recurrence;
pub mod resolve;
pub mod schedule;
pub mod service;
pub mod time_block;

pub use booking::{Booking, BookingId, BookingStatus, NewBooking};
pub use cal_event::CalEvent;
pub use day_plan::{BlockAssignment, DayPlan, PlannedBlock};
pub use error::SchedulingError;
pub use event_type::{EventType, EventTypeId, EventTypeLocation};
pub use resolve::{
    BlockRole, ChangeAction, ReconcileChange, ReconcileOutcome, ResolvedBlock, is_embedded,
    meal_slot_for_block, merge_template, reconcile, role_for,
};
pub use schedule::{
    AvailabilityRule, AvailabilitySchedule, ScheduleId, SlotQuery, TimeSlot, Weekday,
};
pub use service::{
    Bookings, CalendarEvents, DayPlans, DayTemplates, EventTypes, Schedules, SchedulingEvent,
    SchedulingEvents, Slots,
};
pub use time_block::{
    BlockCategory, DayTemplate, DayTemplateId, TimeBlock, TimeBlockId, TimeOfDay,
};

// architect-emitted vox bits. Each capability gets its own
// client / dispatcher / descriptor. Consumer mount sites stitch
// the ones they need into an `architect::Services` bundle.
#[cfg(feature = "vox")]
pub use service::{
    bookings::prelude::*, calendar_events::prelude::*, day_plans::prelude::*,
    day_templates::prelude::*, event_types::prelude::*, schedules::prelude::*, slots::prelude::*,
};

// The slice's one `#[subscribe]` stream — live scheduling changes
// across every sub-resource. Mount
// `service::events::stream_layer(backend)` next to the capability
// services; subscribers drive a `SchedulingEventsStreamClient`.
#[cfg(feature = "vox")]
pub use service::events::{
    SchedulingEventsStream, SchedulingEventsStreamClient, SchedulingEventsStreamSource,
    scheduling_events_stream_service_descriptor as scheduling_events_stream_descriptor,
    stream_layer as scheduling_events_stream_layer,
};
