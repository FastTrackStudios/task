//! Non-CRUD ledger operations.
//!
//! Account + Transaction CRUD comes from the architect-emitted
//! `AccountRepo` + `TransactionRepo` traits. This trait holds
//! the operations that need invariant enforcement (sum-to-zero)
//! or cross-row computation (balances, account history).

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::book::Book;
use crate::error::FinanceError;
use crate::ledger::{Account, Transaction, TransactionSource, TransactionSplits};

/// Balance returned by [`Ledger::balances`].
#[derive(Debug, Clone, PartialEq, facet::Facet)]
pub struct AccountBalance {
    pub account_id: Uuid,
    pub balance_minor: i64,
    pub currency: String,
}

/// Inputs for [`Ledger::post_transaction`]. Architect's emitted
/// `TransactionCreate` is too permissive for posting (you can't
/// hand it raw splits without checking sum=0), so we wrap.
#[derive(Debug, Clone, PartialEq, facet::Facet)]
pub struct PostTransaction {
    pub book_id: Uuid,
    /// ISO-8601 date.
    pub date: String,
    pub description: String,
    pub reference: String,
    pub source_kind: TransactionSource,
    /// Overlay row that owns this transaction. `Uuid::nil()` for
    /// manual entries.
    pub source_id: Uuid,
    pub splits: TransactionSplits,
}

#[architect::rpc]
pub trait Ledger {
    /// Post a journal entry. Backend enforces sum-of-splits = 0
    /// in the transaction's base currency; returns
    /// `SplitsImbalanced` otherwise.
    fn post_transaction(&self, payload: PostTransaction) -> Result<Uuid, FinanceError>;

    /// Recent transactions for an account.
    fn account_transactions(
        &self,
        account_id: Uuid,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<Transaction>, FinanceError>;

    /// Current balance for every account in the book.
    fn balances(
        &self,
        book_id: Uuid,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Vec<AccountBalance>, FinanceError>;

    /// Every book in the org's finance DB. A read surface so clients
    /// (the ledger UI) can resolve a `book_id` without a separate
    /// `BookRepo` mount.
    fn books(&self) -> Result<Vec<Book>, FinanceError>;

    /// Every (non-archived-or-not) account in a book, with names +
    /// kinds. Pairs with [`Ledger::balances`] for the ledger view.
    fn accounts(&self, book_id: Uuid) -> Result<Vec<Account>, FinanceError>;
}
