//! `Client` — the customer being billed. One row per
//! billable counterparty. Multiple [`crate::Engagement`]s can
//! reference the same client.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[architect(table_name = "timer_clients", repo)]
pub struct Client {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Owning organization (`architect_auth::AuthOrganization::id`).
    #[architect(filterable)]
    pub org_id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Free-text contact / billing notes — address, contact
    /// person, payment terms. Not parsed.
    pub notes: String,

    /// `false` once the client is no longer active — kept on
    /// disk for historical timesheet integrity.
    #[architect(filterable)]
    pub archived: bool,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}
