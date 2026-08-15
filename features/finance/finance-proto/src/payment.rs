//! Payments + payment allocation (the InvoiceNinja
//! `paymentables` analog).
//!
//! One payment can be split across many invoices / credits via
//! [`InvoicePayment`] join rows.

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
pub enum PaymentMethod {
    Cash,
    Check,
    BankTransfer,
    CreditCard,
    DebitCard,
    Ach,
    Wire,
    Paypal,
    Other,
}

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
pub enum PaymentStatus {
    Pending,
    Completed,
    Failed,
    Cancelled,
    PartiallyRefunded,
    Refunded,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_payments", repo)]
pub struct Payment {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    #[architect(filterable)]
    pub party_id: Uuid,

    /// ISO-8601 date.
    #[architect(filterable, sortable)]
    pub date: String,

    pub amount_minor: i64,

    #[architect(filterable)]
    pub currency: String,

    pub exchange_rate_micro: i64,

    #[architect(json, filterable)]
    pub method: PaymentMethod,

    /// Bank reference / check number / processor txn id.
    #[architect(filterable, fulltext)]
    pub reference: String,

    #[architect(json, filterable)]
    pub status: PaymentStatus,

    /// Sum of allocations against invoices/credits.
    pub applied_minor: i64,

    pub refunded_minor: i64,

    pub notes: String,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}

/// One allocation row — a payment paid (some of its) amount
/// against one invoice or credit. Its own entity so the join
/// is queryable (e.g. "list payments against this invoice").
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_invoice_payments", repo)]
pub struct InvoicePayment {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub payment_id: Uuid,

    /// Target invoice (or credit — same id space).
    #[architect(filterable)]
    pub invoice_id: Uuid,

    pub amount_minor: i64,
    pub refunded_minor: i64,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}
