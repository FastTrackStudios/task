//! Per-capability traits for the scheduling feature. Same shape
//! as `wiki-proto::service::*` — no umbrella trait; callers
//! express what they need via bounds, so function signatures are
//! self-documenting their requirements:
//!
//! ```ignore
//! // Public booking page needs three capabilities — and only those.
//! fn render_booking_page<S: EventTypes + Slots + Bookings>(s: &S, ...) { /* ... */ }
//!
//! // The personal day-template editor only touches templates.
//! fn save_template<S: DayTemplates>(s: &S, ...) { /* ... */ }
//! ```
//!
//! Each trait is `#[architect::rpc]`-decorated and emits its own
//! async client / dispatcher / descriptor. Backends implement
//! only what they support — a future read-only federation peer
//! may impl just `EventTypes + Slots`; the local `VaultScheduler`
//! implements all five.

pub mod bookings;
pub mod calendar_events;
pub mod day_plans;
pub mod day_templates;
pub mod event_types;
pub mod events;
pub mod schedules;
pub mod slots;

pub use bookings::Bookings;
pub use calendar_events::CalendarEvents;
pub use day_plans::DayPlans;
pub use day_templates::DayTemplates;
pub use event_types::EventTypes;
pub use events::{SchedulingEvent, SchedulingEvents};
pub use schedules::Schedules;
pub use slots::Slots;
