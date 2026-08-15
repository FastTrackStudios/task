//! Bookings — instances of someone reserving a slot for an event
//! type.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::event_type::EventTypeId;

/// Stable id for a booking (uuid v4 string). Transparent
/// `JsonField` so it round-trips as a plain string in JSON
/// columns and on the wire.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct BookingId(pub String);

impl From<String> for BookingId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum BookingStatus {
    /// Awaiting host confirmation. Default for new bookings on
    /// event types that have `requires_confirmation`.
    #[default]
    Pending,
    /// Host accepted or the event type auto-confirms.
    Confirmed,
    /// Cancelled by either side.
    Cancelled,
    /// Marked as no-show after the slot end.
    NoShow,
    /// The event happened.
    Completed,
}

/// A booking the public page is creating. The host's vault stores
/// it as a markdown file in `scheduling/bookings/`.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct NewBooking {
    pub event_type_id: EventTypeId,
    /// ISO-8601 UTC inclusive start.
    pub start_utc: String,
    /// ISO-8601 UTC exclusive end. The host validates that
    /// `end - start` matches the event type's duration; the proto
    /// lets you over-specify so future variable-duration event
    /// types still fit.
    pub end_utc: String,
    pub attendee_name: String,
    pub attendee_email: String,
    /// Free-form note the bookee left on the booking form.
    pub note: Option<String>,
}

/// A persisted booking. The vault markdown file is the source of
/// truth; the SeaORM mirror emitted under `--features server`
/// keeps reverse lookups (by event type, by attendee, by date
/// range) index-fast.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "scheduling_bookings", repo)]
pub struct Booking {
    /// Vault path of the markdown file (set by the scanner). PK.
    #[architect(primary_key, auto_increment = false)]
    pub path: String,
    #[architect(json, filterable)]
    pub id: BookingId,
    #[architect(json, filterable)]
    pub event_type_id: EventTypeId,
    #[architect(filterable, sortable)]
    pub start_utc: String,
    #[architect(filterable, sortable)]
    pub end_utc: String,
    #[architect(filterable, fulltext)]
    pub attendee_name: String,
    #[architect(filterable, fulltext)]
    pub attendee_email: String,
    pub note: Option<String>,
    #[architect(json, filterable)]
    pub status: BookingStatus,
    /// ISO-8601 UTC creation timestamp.
    #[architect(filterable, sortable)]
    pub created_utc: String,
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable booking id (`id.0`), minted at create time --
/// the vault `path` PK isn't known until the scanner assigns it.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Booking {
    type Key = String;
    fn key(&self) -> String {
        self.id.0.clone()
    }
}
