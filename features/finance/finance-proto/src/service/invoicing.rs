//! Non-CRUD invoicing workflow operations.
//!
//! Invoice / Payment / RecurringSchedule CRUD comes from the
//! architect-emitted `InvoiceRepo` / `PaymentRepo` /
//! `RecurringScheduleRepo` traits.

use uuid::Uuid;

use crate::error::FinanceError;
use crate::invoice::Invoice;

/// One allocation row for [`Invoicing::record_payment`].
#[derive(Debug, Clone, PartialEq, facet::Facet)]
pub struct PaymentAllocation {
    pub invoice_id: Uuid,
    pub amount_minor: i64,
}

/// Inputs for [`Invoicing::record_payment`]. Wraps the
/// `PaymentCreate` shape with the allocation list so the call
/// is one-shot.
#[derive(Debug, Clone, PartialEq, facet::Facet)]
pub struct RecordPayment {
    pub book_id: Uuid,
    pub party_id: Uuid,
    /// ISO-8601 date.
    pub date: String,
    pub amount_minor: i64,
    pub currency: String,
    pub exchange_rate_micro: i64,
    pub method: crate::payment::PaymentMethod,
    pub reference: String,
    pub notes: String,
    pub allocations: Vec<PaymentAllocation>,
}

/// Inputs for [`Invoicing::generate_invoice`] — build a draft invoice
/// from a project's billable, not-yet-invoiced timer sessions.
#[derive(Debug, Clone, PartialEq, facet::Facet)]
pub struct GenerateInvoice {
    /// Project bucket → `Some(id)`. Tag / general bucket → `None`.
    pub project_id: Option<Uuid>,
    /// Tag bucket → the tag name (bills project-less time with that
    /// tag). General bucket → empty (bills project-less, untagged time).
    /// Ignored when `project_id` is set.
    pub tag: String,
    /// Bill-to display name. Find-or-creates a party in the org's book.
    pub client_name: String,
    /// Inclusive ISO `YYYY-MM-DD` lower bound, or empty for no bound.
    pub since: String,
    /// Inclusive ISO `YYYY-MM-DD` upper bound, or empty for no bound.
    pub until: String,
    /// `due_date = issue_date + net_days`.
    pub net_days: i64,
}

/// A bucket of billable time not yet invoiced. Either a **project**
/// (`project_id = Some`), a **tag** of project-less time
/// (`tag` non-empty), or **general** project-less, untagged time
/// (`project_id = None`, `tag` empty).
#[derive(Debug, Clone, PartialEq, facet::Facet)]
pub struct UninvoicedGroup {
    pub project_id: Option<Uuid>,
    pub tag: String,
    pub session_count: i64,
    /// Total seconds.
    pub seconds: i64,
    pub amount_minor: i64,
    pub currency: String,
}

#[architect::rpc]
pub trait Invoicing {
    /// Build + persist a **draft** invoice from a project's billable,
    /// not-yet-invoiced sessions, marking those sessions as billed.
    fn generate_invoice(&self, req: GenerateInvoice) -> Result<Invoice, FinanceError>;

    /// All invoices in the org's book, newest first.
    fn list_invoices(&self) -> Result<Vec<Invoice>, FinanceError>;

    /// One invoice by id.
    fn get_invoice(&self, id: Uuid) -> Result<Invoice, FinanceError>;

    /// Delete a **draft** invoice and un-bill its sessions. Issued
    /// (non-draft) invoices must be voided instead.
    fn delete_invoice(&self, id: Uuid) -> Result<(), FinanceError>;

    /// Record a payment of `amount_minor` against one invoice and
    /// update its paid / balance / status. Returns the updated invoice.
    fn record_invoice_payment(
        &self,
        id: Uuid,
        amount_minor: i64,
        date: String,
    ) -> Result<Invoice, FinanceError>;

    /// Per-project billable time not yet on any invoice.
    fn uninvoiced(&self) -> Result<Vec<UninvoicedGroup>, FinanceError>;

    /// Issue an invoice: assigns a number from the book's
    /// counter, locks the line items, posts the AR ↔ income
    /// transaction to the ledger.
    fn mark_sent(&self, id: Uuid) -> Result<Invoice, FinanceError>;

    /// Reverse an issued invoice via a credit note. Returns the
    /// new credit's id.
    fn void_with_credit(&self, id: Uuid, reason: String) -> Result<Uuid, FinanceError>;

    /// Record a payment and allocate it across invoices/credits
    /// in one transaction. Sum of allocations must be ≤
    /// `amount_minor`; remainder becomes an open credit on the
    /// party.
    fn record_payment(&self, payload: RecordPayment) -> Result<Uuid, FinanceError>;

    /// Refund (full or partial).
    fn refund_payment(
        &self,
        id: Uuid,
        amount_minor: i64,
        reason: String,
    ) -> Result<(), FinanceError>;

    /// Manually trigger the generator for a single schedule.
    /// Returns the generated invoice's id, or `None` if not due
    /// / paused / completed.
    fn run_schedule_once(&self, id: Uuid) -> Result<Option<Uuid>, FinanceError>;

    /// Persist a **client-built** invoice and stamp its source
    /// sessions as billed — the counterpart of
    /// [`Invoicing::generate_invoice`] for callers that assemble
    /// the invoice themselves (the CLI's `task finance invoice`
    /// numbering + PDF flow). Upserts `book` / `party`
    /// (insert-if-missing by id), rejects a duplicate invoice
    /// number, inserts the invoice as-is, and sets
    /// `work_sessions.invoice_id` on `session_ids`. Returns the
    /// number of sessions stamped.
    fn commit_invoice(
        &self,
        book: crate::book::Book,
        party: crate::party::Party,
        invoice: Invoice,
        session_ids: Vec<Uuid>,
    ) -> Result<u64, FinanceError>;

    /// Plain void (no credit note): set the invoice to
    /// `Cancelled` and un-stamp its sessions so the hours become
    /// re-billable. Refuses when payments have been recorded
    /// against it — issue a credit note instead
    /// ([`Invoicing::void_with_credit`]). Returns the number of
    /// sessions un-stamped.
    fn void_invoice(&self, id: Uuid) -> Result<u64, FinanceError>;
}
