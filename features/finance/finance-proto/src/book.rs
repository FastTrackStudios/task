//! Books — the tenancy root.
//!
//! One user may own many books. A book is the unit of:
//! - Number sequences (invoice / quote / credit / expense numbers
//!   are unique per `(book_id, kind)`).
//! - Default currency.
//! - Privacy boundary in the vault export (each book gets its
//!   own `Records/Finance/{book}/` subtree).
//!
//! "Personal" vs "business" is a soft hint — both kinds share
//! the same schema; the flag drives UI defaults (hide
//! vendors/clients/invoicing UI for personal books).

use chrono::{DateTime, Utc};
use facet::Facet;
use uuid::Uuid;

/// Personal vs business book. JSON-stored via
/// `architect::JsonField`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Facet,
    serde::Serialize,
    serde::Deserialize,
)]
#[repr(u8)]
pub enum BookKind {
    /// Personal finance. UI surfaces ledger + expenses + budgets;
    /// hides AR/invoicing by default.
    Personal,
    /// Business / freelance / side-gig. Full surface.
    Business,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_books", repo)]
pub struct Book {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Personal vs business — stored as JSON because architect's
    /// derive doesn't have a native enum mapping.
    #[architect(json, filterable)]
    pub kind: BookKind,

    /// ISO 4217 default currency (e.g. "USD").
    #[architect(filterable)]
    pub base_currency: String,

    /// Free-form settings JSON (invoice numbering format, fiscal
    /// year start, default tax catalog).
    pub settings_json: String,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}
