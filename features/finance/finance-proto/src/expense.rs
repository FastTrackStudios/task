//! Expenses — AP side, and the primary entry surface for
//! personal users.
//!
//! Every expense posts to the GL via a [`crate::Transaction`].

use chrono::{DateTime, Utc};
use facet::Facet;
use uuid::Uuid;

use crate::tax::TaxLines;

/// Newtype for storing `Vec<String>` tags in a JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField,
    Debug,
    Clone,
    Default,
    PartialEq,
    Facet,
    serde::Serialize,
    serde::Deserialize,
)]
#[repr(transparent)]
pub struct Tags(pub Vec<String>);

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_expenses", repo)]
pub struct Expense {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    /// Vendor (business) or `Uuid::nil()` for personal expenses.
    #[architect(filterable)]
    pub party_id: Uuid,

    /// Category account — what GL bucket this hits. Always an
    /// account of `kind = Expense`.
    #[architect(filterable)]
    pub category_account_id: Uuid,

    /// Payment-source account (cash / credit card / checking).
    /// Always an asset or liability account.
    #[architect(filterable)]
    pub payment_account_id: Uuid,

    /// ISO-8601 date.
    #[architect(filterable, sortable)]
    pub date: String,

    pub amount_minor: i64,

    #[architect(filterable)]
    pub currency: String,

    pub exchange_rate_micro: i64,

    #[architect(json)]
    pub taxes: TaxLines,

    /// Receipt #, merchant name.
    #[architect(filterable, fulltext)]
    pub reference: String,

    #[architect(fulltext)]
    pub notes: String,

    /// If non-nil, this expense is queued for inclusion on this
    /// invoice as a `LineItemType::Expense` row.
    #[architect(filterable)]
    pub billable_to_invoice_id: Uuid,

    /// Attachment id (receipt photo/PDF) owned by the
    /// `attachments` feature. `Uuid::nil()` = none.
    pub receipt_attachment_id: Uuid,

    #[architect(json, filterable)]
    pub tags: Tags,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}
