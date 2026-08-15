//! General ledger types: accounts, transactions, splits.
//!
//! Every monetary event posts to the ledger as a [`Transaction`]
//! whose [`TransactionSplit`]s sum to zero (in base currency).
//! This is the authoritative truth; invoices / payments /
//! expenses are presentational overlays that reference the
//! underlying transaction.

use chrono::{DateTime, Utc};
use facet::Facet;
use uuid::Uuid;

/// Top-level account category.
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
pub enum AccountKind {
    /// Cash, checking, savings, AR, prepaid expenses.
    Asset,
    /// Credit cards, loans, AP, taxes payable.
    Liability,
    /// Owner's equity, retained earnings.
    Equity,
    /// Sales, interest income, refunds-received.
    Income,
    /// Operating expenses + cost of goods. Personal users see
    /// these as "categories."
    Expense,
}

/// What kind of overlay row created this transaction.
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
pub enum TransactionSource {
    Manual,
    Invoice,
    Payment,
    Expense,
    Recurring,
    Import,
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_accounts", repo)]
pub struct Account {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    #[architect(json, filterable)]
    pub kind: AccountKind,

    /// Tree parent — `Uuid::nil()` for top-level accounts.
    #[architect(filterable)]
    pub parent_id: Uuid,

    /// ISO 4217. Empty = inherit from book.
    #[architect(filterable)]
    pub currency: String,

    #[architect(filterable)]
    pub is_archived: bool,

    /// Opening balance in minor units (signed; debit-positive
    /// for asset/expense, credit-positive for liability/equity/
    /// income).
    pub opening_balance_minor: i64,

    pub notes: String,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

/// One side of a journal entry. Stored as JSON inside
/// [`Transaction::splits`]. Not its own entity — splits are
/// always retrieved with their parent transaction.
///
/// Sign convention: positive `amount_minor` is debit, negative
/// is credit. Sum of all splits in a transaction = 0 in base
/// currency.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Facet, serde::Serialize, serde::Deserialize,
)]
pub struct TransactionSplit {
    pub account_id: Uuid,
    /// Signed minor units in the split's currency.
    pub amount_minor: i64,
    /// Same amount converted into the transaction's base
    /// currency. Equal to `amount_minor` for same-currency
    /// splits.
    pub fx_amount_minor: i64,
    /// FX rate in micro-units (1.0875 USD/EUR → 10_875_000).
    /// `1_000_000` for same-currency splits.
    pub fx_rate_micro: i64,
    pub memo: String,
}

/// Newtype for storing `Vec<TransactionSplit>` in a JSON column.
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
pub struct TransactionSplits(pub Vec<TransactionSplit>);

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Facet, Clone, Debug, PartialEq)]
#[architect(table_name = "finance_transactions", repo)]
pub struct Transaction {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    #[architect(filterable)]
    pub book_id: Uuid,

    /// ISO-8601 date (no time — posting date).
    #[architect(filterable, sortable)]
    pub date: String,

    #[architect(fulltext)]
    pub description: String,

    /// Free-form reference (check number, bank-statement id,
    /// receipt id).
    #[architect(filterable, fulltext)]
    pub reference: String,

    /// What created this transaction.
    #[architect(json, filterable)]
    pub source_kind: TransactionSource,

    /// Id of the overlay row (invoice / payment / expense) that
    /// owns this transaction. `Uuid::nil()` for manual entries.
    #[architect(filterable)]
    pub source_id: Uuid,

    /// Splits live inline as JSON. Backend enforces sum-to-zero
    /// invariant on write.
    #[architect(json)]
    pub splits: TransactionSplits,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}
