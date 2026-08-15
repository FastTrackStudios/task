//! Service-level errors for the finance feature.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, Error, Facet)]
#[repr(u8)]
pub enum FinanceError {
    #[error("not found: {id}")]
    NotFound { id: String },
    /// Double-entry violation: the splits of a transaction do not
    /// sum to zero in the transaction's base currency.
    #[error("transaction splits do not sum to zero (off by {off_by_minor})")]
    SplitsImbalanced { off_by_minor: i64 },
    /// Tried to edit an invoice that's been issued (and is now
    /// locked). Use credit notes / void instead.
    #[error("invoice {id} is locked")]
    InvoiceLocked { id: String },
    /// Invoice number collides with an existing one in the same
    /// `(book_id, kind)` namespace.
    #[error("duplicate invoice number {number} in book {book_id}")]
    DuplicateInvoiceNumber { book_id: String, number: String },
    /// Payment allocation exceeds the invoice's remaining balance.
    #[error("payment allocation exceeds remaining balance ({applied_minor} > {remaining_minor})")]
    OverAllocation {
        applied_minor: i64,
        remaining_minor: i64,
    },
    /// Validation failure on a payload field. Dotted-path
    /// (`invoice.line_items[2].quantity`).
    #[error("invalid {field}: {reason}")]
    Invalid { field: String, reason: String },
    /// Currency mismatch: a multi-currency operation was given
    /// rows whose currencies don't match and no FX rate was
    /// supplied.
    #[error("currency mismatch: {expected} vs {actual}")]
    CurrencyMismatch { expected: String, actual: String },
    /// Backend storage IO bubbled up.
    #[error("backend: {message}")]
    Backend { message: String },
}
