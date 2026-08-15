//! Tax line items.
//!
//! InvoiceNinja-style: tax names + rates are *copied* into each
//! invoice / line item so historical rate changes don't rewrite
//! existing books. The catalog table is autocomplete-only.

use chrono::{DateTime, Utc};
use facet::Facet;
use uuid::Uuid;

/// A single tax line attached to a line item or to an invoice
/// as a whole. Lives in JSON columns; not its own entity.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Facet, serde::Serialize, serde::Deserialize,
)]
pub struct TaxLine {
    /// Display name ("VAT 20%", "Sales Tax CA", "GST").
    pub name: String,
    /// Rate as micro-percent (20% = 200_000; 8.875% = 88_750).
    pub rate_micro: i64,
    /// Computed tax amount in minor units (denormalized so the
    /// JSON snapshot is self-describing).
    pub amount_minor: i64,
}

/// Newtype wrapper for storing `Vec<TaxLine>` in a JSON column.
/// Orphan rules forbid `From<Vec<X>> for sea_orm::Value`, so we
/// wrap. Used on both [`crate::Invoice::invoice_taxes`] and
/// [`crate::Expense::taxes`].
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
pub struct TaxLines(pub Vec<TaxLine>);

/// One entry in a book's tax-rate autocomplete catalog. Not a
/// FK target — invoices copy `name` + `rate_micro` into their
/// JSON.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_tax_rate_catalog", repo)]
pub struct TaxRateCatalogEntry {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    pub rate_micro: i64,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}
