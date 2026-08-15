//! Invoicing: invoices, quotes, credits, recurring templates.
//!
//! All four variants share one table, discriminated by
//! [`InvoiceKind`]. Line items + per-invoice taxes are
//! denormalized into JSON columns; once `locked = true` they
//! are frozen — edits go through credit notes / voids.

use chrono::{DateTime, Utc};
use facet::Facet;
use uuid::Uuid;

use crate::tax::{TaxLine, TaxLines};

/// Which sales-document variant this row represents.
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
pub enum InvoiceKind {
    Invoice,
    Quote,
    /// Credit note — negative-balance document that offsets an
    /// invoice.
    Credit,
    /// Template that generates real invoices on a schedule.
    RecurringTemplate,
}

/// Lifecycle.
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
pub enum InvoiceStatus {
    Draft,
    Sent,
    Viewed,
    PartiallyPaid,
    Paid,
    Overdue,
    Cancelled,
    /// Issued but reversed by a credit note.
    Reversed,
}

/// What this line item represents. Mirrors InvoiceNinja's
/// `type_id` on `InvoiceItem`.
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
pub enum LineItemType {
    Product,
    Service,
    /// Time entry rolled into this line. The source entry id
    /// lives in `source_ref`.
    TimeEntry,
    /// Reimbursable expense.
    Expense,
    /// Late fee / interest.
    LateFee,
    /// Gateway / payment-processing fee passed through.
    GatewayFee,
}

/// One row on an invoice. Lives in the invoice's `line_items`
/// JSON column — not its own table.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Facet, serde::Serialize, serde::Deserialize,
)]
pub struct InvoiceLineItem {
    /// Stable client-side id so drag-reorder doesn't reassign.
    pub id: Uuid,
    pub kind: LineItemType,
    pub description: String,
    /// Quantity in millis (3 decimal places). 1.5 → 1_500.
    pub quantity_milli: i64,
    pub unit_price_minor: i64,
    /// Per-line discount in minor units (applied before tax).
    pub discount_minor: i64,
    /// Up to 3 tax lines (UI convention). Plain `Vec<TaxLine>`
    /// here — we're inside another JSON-stored struct, not
    /// inside an entity column, so the orphan-rule wrapping
    /// trick isn't needed.
    pub taxes: Vec<TaxLine>,
    /// Computed `quantity * unit_price - discount`. Denormalized
    /// so the snapshot is self-describing.
    pub line_total_minor: i64,
    /// Source row reference (time entry id, expense id, …).
    /// `Uuid::nil()` for free-typed lines.
    pub source_ref: Uuid,
}

/// Newtype for storing `Vec<InvoiceLineItem>` in a JSON column.
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
pub struct InvoiceLineItems(pub Vec<InvoiceLineItem>);

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_invoices", repo)]
pub struct Invoice {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    #[architect(filterable)]
    pub party_id: Uuid,

    #[architect(json, filterable)]
    pub kind: InvoiceKind,

    /// User-visible number ("INV-0042"). Empty until issued
    /// (matches InvoiceNinja — drafts have no number).
    #[architect(filterable, sortable, fulltext)]
    pub number: String,

    #[architect(json, filterable)]
    pub status: InvoiceStatus,

    /// ISO-8601 date.
    #[architect(filterable, sortable)]
    pub issue_date: String,

    /// ISO-8601 date. Empty = no due date set.
    #[architect(filterable, sortable)]
    pub due_date: String,

    #[architect(filterable)]
    pub currency: String,

    /// FX rate against the book's base currency (1.0875 →
    /// 10_875_000). Captured at issuance.
    pub exchange_rate_micro: i64,

    /// Snapshot of line items. Frozen once `locked = true`.
    #[architect(json)]
    pub line_items: InvoiceLineItems,

    /// Per-invoice tax lines (in addition to per-line taxes).
    #[architect(json)]
    pub invoice_taxes: TaxLines,

    /// Treat per-line `unit_price_minor` as tax-inclusive when
    /// true (EU-style); else tax-exclusive (US-style).
    pub uses_inclusive_taxes: bool,

    pub subtotal_minor: i64,
    pub tax_total_minor: i64,
    pub total_minor: i64,
    pub amount_paid_minor: i64,
    /// `total_minor - amount_paid_minor`. Denormalized.
    #[architect(sortable)]
    pub balance_minor: i64,

    pub notes_public: String,
    pub notes_private: String,
    pub terms: String,
    pub footer: String,

    /// `true` once issued. Blocks edits to amounts / line items.
    #[architect(filterable)]
    pub locked: bool,

    /// Posting timestamp (when status moved to `Sent`).
    /// Sentinel: `Utc.timestamp_opt(0, 0)` = not yet posted.
    pub posted_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}
