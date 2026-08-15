//! Parties — unified clients + vendors.
//!
//! InvoiceNinja splits these across `clients` and `vendors`
//! tables. We unify: most real-world counterparties play both
//! roles, and personal-finance contacts don't fit either role
//! cleanly. The [`PartyKind`] flag enables/disables AR/AP UI
//! per row.

use chrono::{DateTime, Utc};
use facet::Facet;
use uuid::Uuid;

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
pub enum PartyKind {
    /// AR-only: we invoice them.
    Client,
    /// AP-only: we owe them / pay them.
    Vendor,
    /// Both AR and AP.
    Both,
    /// Yourself / your own businesses (for inter-entity
    /// transfers across books).
    SelfEntity,
}

/// Filter role for list queries (not stored on the row, so no
/// `JsonField` derive needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum PartyRole {
    Receivable,
    Payable,
    Any,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_parties", repo)]
pub struct Party {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    #[architect(json, filterable)]
    pub kind: PartyKind,

    #[architect(filterable, sortable, fulltext)]
    pub display_name: String,

    /// Legal name when different from `display_name` (used on
    /// invoices/PDF).
    #[architect(fulltext)]
    pub legal_name: String,

    #[architect(filterable, fulltext)]
    pub email: String,

    pub phone: String,

    /// Free-form address (multi-line). Renderers split on `\n`.
    pub address: String,

    /// Tax identifier (EIN / VAT / ABN).
    pub tax_id: String,

    /// ISO 4217 default invoicing currency. Empty = inherit
    /// from book.
    #[architect(filterable)]
    pub default_currency: String,

    /// Default payment-terms days (NET-30 → 30). 0 = inherit.
    pub default_net_days: i32,

    /// Default billable hourly rate in minor units. 0 = unset.
    pub default_rate_minor_per_hour: i64,

    pub notes: String,

    #[architect(filterable)]
    pub is_archived: bool,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}
