//! Cal.com-style bookable event type.
//!
//! Mirrors the [cal.com `EventType` model][cal] surface that
//! matters for booking: title / slug / duration / location /
//! optional schedule reference. Things we *don't* carry on the
//! wire (recurring meetings, payment, custom fields) live behind
//! `metadata` so the proto stays stable while the UI evolves.
//!
//! [cal]: https://github.com/calcom/cal.com/blob/main/packages/prisma/schema.prisma

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::schedule::ScheduleId;

/// Stable id for an event type (uuid v4 string).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct EventTypeId(pub String);

impl From<String> for EventTypeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Where the meeting happens. Inline so we can drop the dependency
/// on a Location service for the v1 cut.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum EventTypeLocation {
    /// Phone call — number arrives in `metadata.phone` at booking time.
    Phone,
    /// In-person — `address` is shown to the bookee.
    InPerson { address: String },
    /// Generic conferencing URL the host pastes in. Future work
    /// adds first-party Zoom / Meet / Whereby integrations.
    Link { url: String },
    /// "We'll decide later" — the host's choice during prep.
    #[default]
    Tbd,
}

/// One bookable thing: a 30-minute consultation, a 60-minute
/// strategy session, etc.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "scheduling_event_types", repo)]
pub struct EventType {
    /// Vault-relative path of the markdown file backing this event
    /// type. Populated by the scanner; not part of the wire write. PK.
    #[architect(primary_key, auto_increment = false)]
    pub path: String,
    #[architect(json, filterable)]
    pub id: EventTypeId,
    #[architect(filterable, sortable, fulltext)]
    pub title: String,
    /// URL-safe slug. The public booking page lives at
    /// `/book/<owner>/<slug>`.
    #[architect(filterable, sortable)]
    pub slug: String,
    #[architect(fulltext)]
    pub description: Option<String>,
    /// Duration in minutes.
    #[architect(filterable, sortable)]
    pub duration_min: u16,
    /// How many minutes between consecutive bookings. Default 0;
    /// useful when the host needs a buffer.
    pub buffer_min: u16,
    #[architect(json)]
    pub location: EventTypeLocation,
    /// Which availability schedule controls open slots. `None`
    /// means "use the owner's default schedule".
    #[architect(json, filterable)]
    pub schedule_id: Option<ScheduleId>,
    /// True if the event type is published to the public booking
    /// page. Defaults true for new event types.
    #[architect(filterable)]
    pub published: bool,
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable event-type id (`id.0`), minted at create time
/// -- the vault `path` PK isn't known until the scanner assigns it.
#[cfg(feature = "atom")]
impl architect::StoreEntity for EventType {
    type Key = String;
    fn key(&self) -> String {
        self.id.0.clone()
    }
}
