#![allow(clippy::large_futures)]
//! `finance-db` — SeaORM + SQLite, authoritative finance store.
//!
//! Entity items come from `finance-proto` via its `server`
//! feature (`architect::Entity` emits the SeaORM model inside
//! a hidden `__<snake>_storage` module per entity). This
//! crate exposes the [`Migrator`] that creates the matching
//! tables, plus an [`entity`] module re-exporting the
//! architect-emitted items under stable names.
//!
//! Schema highlights:
//!
//! - `finance_books` — tenancy root.
//! - `finance_accounts` — GL accounts (asset/liability/
//!   equity/income/expense), tree via `parent_id`.
//! - `finance_transactions` — header row; splits live as a
//!   JSON column on the same row (matches the proto's
//!   `TransactionSplits` newtype). Sum-to-zero is enforced
//!   in the service layer, not as a DB constraint.
//! - `finance_parties` — unified clients + vendors.
//! - `finance_invoices` — single table covering invoice /
//!   quote / credit / recurring-template variants;
//!   line-items + tax-lines snapshot as JSON columns.
//! - `finance_payments` + `finance_invoice_payments` —
//!   polymorphic allocation.
//! - `finance_recurring_schedules` — invoice template runner.
//! - `finance_expenses` — AP side.
//! - `finance_tax_rate_catalog` — reusable tax rates per book.

pub mod entity;
pub mod migrations;

pub use migrations::Migrator;
