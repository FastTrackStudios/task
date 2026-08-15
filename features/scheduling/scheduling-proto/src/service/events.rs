//! One live event stream for the whole scheduling slice.
//!
//! The slice is seven capability sub-traits, but the *live* surface
//! is one stream: a calendar page overlays day plans, calendar
//! events, and bookings at once, so it wants one subscription — not
//! seven — and the event names which sub-resource changed. Backends
//! that implement any of the write capabilities publish into the one
//! hub behind [`SchedulingEvents`].

use crate::booking::Booking;
use crate::cal_event::CalEvent;
use crate::day_plan::DayPlan;
use crate::event_type::EventType;
use crate::schedule::AvailabilitySchedule;
use crate::time_block::DayTemplate;

/// One scheduling change, broadcast to every [`SchedulingEvents`]
/// subscriber on each successful mutation anywhere in the slice.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// The stream carries *changes only* — there is no `Snapshot`
/// variant. A subscriber fetches the sub-resource lists it renders
/// once (after subscribing, so nothing is missed in between) via the
/// per-capability list verbs (`get_day_plan` / `list_events` /
/// `list_bookings` / …), then folds:
///
/// - every `*Upserted` variant carries the **full post-write**
///   record — replace (or insert) the row with the matching key.
///   Idempotent re-application is harmless. Booking status changes
///   surface as [`SchedulingEvent::BookingUpserted`] with the
///   post-write row (bookings are never deleted, only
///   status-transitioned).
/// - every `*Deleted` variant carries the sub-resource's key
///   (template/schedule/event-type id, day-plan date, event id) —
///   remove that row.
///
/// Subscribers that render only one sub-resource ignore the other
/// variants — the stream is unfiltered by design (one subscription
/// per client; filtering is client-side, like `TaskEvent`).
#[derive(
    Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, ::facet::Facet,
)]
#[repr(u8)]
// Upserted variants carry full records by design (idempotent
// full-state payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum SchedulingEvent {
    /// A day template was created or replaced.
    DayTemplateUpserted(DayTemplate),
    /// The day template with this id was removed.
    DayTemplateDeleted(String),
    /// A per-date plan was saved (whole-day replace).
    DayPlanUpserted(DayPlan),
    /// The plan for this ISO `YYYY-MM-DD` date was dropped
    /// (the date reverts to its template).
    DayPlanDeleted(String),
    /// A concrete calendar event was created or replaced.
    CalendarEventUpserted(CalEvent),
    /// The calendar event with this id was removed.
    CalendarEventDeleted(String),
    /// A bookable event type was created or replaced.
    EventTypeUpserted(EventType),
    /// The event type with this id was removed.
    EventTypeDeleted(String),
    /// An availability schedule was created or replaced.
    ScheduleUpserted(AvailabilitySchedule),
    /// The schedule with this id was removed.
    ScheduleDeleted(String),
    /// A booking was created or its status changed — the payload is
    /// the complete post-write row.
    BookingUpserted(Booking),
}

/// The slice's one `#[subscribe]` stream — see [`SchedulingEvent`]
/// for the fetch-once-then-fold subscriber contract. Subscribe-only
/// (like `agent-proto`'s `Subscriptions`): the read/write verbs live
/// on the per-capability traits.
#[architect::rpc]
pub trait SchedulingEvents {
    /// Every scheduling change, as it happens — day templates, day
    /// plans, calendar events, event types, schedules, bookings.
    #[subscribe]
    fn events(&self) -> SchedulingEvent;
}
