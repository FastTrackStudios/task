//! Capability traits for non-CRUD workflow operations.
//!
//! Standard list/get/create/update CRUD comes for free from
//! the architect-emitted `<Entity>Repo` traits (`BookRepo`,
//! `AccountRepo`, `PartyRepo`, `InvoiceRepo`, `PaymentRepo`,
//! `ExpenseRepo`, `RecurringScheduleRepo`, etc.). Mount those
//! alongside these workflow traits in the server bundle.
//!
//! These traits hold operations that don't fit CRUD:
//! - **Ledger** — `post_transaction` (sum-to-zero invariant),
//!   `balances`, `account_transactions`.
//! - **Invoicing** — `mark_sent` (assign number + lock + post
//!   to ledger), `void_with_credit`, `record_payment` (with
//!   allocation), `refund_payment`, `run_schedule_once`.
//! - **Expenses** — none yet; quick-add is a `Create` and posts
//!   to the ledger as a side effect inside the repo storage.
//!
//! No umbrella trait — callers express what they need via
//! bounds.

pub mod invoicing;
pub mod ledger;

pub use invoicing::Invoicing;
pub use ledger::Ledger;
