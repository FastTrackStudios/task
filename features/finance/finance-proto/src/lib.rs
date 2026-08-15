//! Wire contract for the finance feature.
//!
//! ## Two macro layers
//!
//! 1. **`#[derive(architect::Entity)]`** on every persisted
//!    struct — emits the wire type, the SeaORM `Model` /
//!    `Entity` / `Column` / `Relation` / `ActiveModel` (under
//!    the `server` feature), the `<Name>Create` / `<Name>Update`
//!    / `<Name>List` payload types, and an auto-CRUD
//!    `<Name>Repo` vox-service trait. `finance-db` re-exports
//!    the storage items.
//!
//! 2. **`#[architect::rpc]`** capability traits in
//!    [`service`] — non-CRUD workflow ops only
//!    (`post_transaction`, `mark_sent`, `record_payment`,
//!    `run_schedule_once`, `balances`, …). Function signatures
//!    are self-documenting via bounds:
//!
//!    ```ignore
//!    fn invoice_editor<S: Invoicing + Ledger>(s: &S) {}
//!    fn ledger_view   <S: Ledger>             (s: &S) {}
//!    ```
//!
//! ## Money on the wire
//!
//! Integer minor units (`i64`) + ISO 4217 `String`. Decimal
//! math happens server-side; the wire stays serializer-agnostic
//! so wasm clients don't drag in `rust_decimal`.
//!
//! ## Dates
//!
//! `chrono::DateTime<Utc>` for timestamps (so the architect
//! derive can map directly to SeaORM `timestamp_with_time_zone`).
//! ISO-8601 `String` for date-only fields (posting date, due
//! date, etc.) where time-of-day is meaningless.

pub mod book;
pub mod error;
pub mod expense;
pub mod invoice;
pub mod ledger;
pub mod party;
pub mod payment;
pub mod recurring;
pub mod service;
pub mod tax;

pub use book::{Book, BookKind};
pub use error::FinanceError;
pub use expense::Expense;
pub use expense::Tags;
pub use invoice::{
    Invoice, InvoiceKind, InvoiceLineItem, InvoiceLineItems, InvoiceStatus, LineItemType,
};
pub use ledger::{
    Account, AccountKind, Transaction, TransactionSource, TransactionSplit, TransactionSplits,
};
pub use party::{Party, PartyKind, PartyRole};
pub use payment::{InvoicePayment, Payment, PaymentMethod, PaymentStatus};
pub use recurring::{RecurringFrequency, RecurringSchedule, RecurringStatus};
pub use service::invoicing::{GenerateInvoice, PaymentAllocation, RecordPayment, UninvoicedGroup};
pub use service::ledger::{AccountBalance, PostTransaction};
pub use service::{Invoicing, Ledger};
pub use tax::{TaxLine, TaxLines, TaxRateCatalogEntry};

#[cfg(feature = "vox")]
pub use service::{
    invoicing::{
        InvoicingClient, InvoicingRpc, InvoicingRpcDispatcher, Service as InvoicingService,
        layer as invoicing_layer, serve as invoicing_serve,
    },
    ledger::{
        LedgerClient, LedgerRpc, LedgerRpcDispatcher, Service as LedgerService,
        layer as ledger_layer, serve as ledger_serve,
    },
};
