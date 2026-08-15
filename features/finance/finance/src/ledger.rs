//! Concrete [`Ledger`] capability service over `finance-db`.
//!
//! The proto trait ([`finance_proto::Ledger`]) is **synchronous**
//! — the `#[architect::rpc]` macro marshals it onto vox through a
//! [`Dispatcher`], so the user-facing impl stays sync and blocks on
//! the async SeaORM work inside. We mirror the `email-jmap` backend:
//! hold a [`tokio::runtime::Handle`] and `block_on` each DB op.
//!
//! ## The balance invariant
//!
//! A journal entry is only valid when its splits sum to zero in the
//! transaction's **base currency**. Each `TransactionSplit` carries
//! both `amount_minor` (in the split's own currency) and
//! `fx_amount_minor` (that amount converted into the transaction's
//! base currency). The invariant is enforced on the base-currency
//! field — `sum(fx_amount_minor) == 0`. A non-zero sum is rejected
//! with [`FinanceError::SplitsImbalanced`] and **nothing is written**.

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use finance_proto::book::Book;
use finance_proto::error::FinanceError;
use finance_proto::ledger::{Account, AccountKind, Transaction};
use finance_proto::service::ledger::{AccountBalance, Ledger, PostTransaction};

use crate::finance_db_entity::{
    AccountActive, AccountColumn, AccountEntity, AccountModel, BookEntity, BookModel,
    TransactionActive, TransactionColumn, TransactionEntity,
};

/// Default page size for [`Ledger::account_transactions`] when the
/// caller passes `limit == 0` (treat as "no explicit cap" → use a
/// sane ceiling rather than returning nothing).
const DEFAULT_TXN_LIMIT: u64 = 100;

/// Concrete ledger service. Cheap to clone — the connection handle
/// and runtime handle are both `Clone`.
#[derive(Clone, architect::HasDispatcher)]
pub struct LedgerService {
    db: DatabaseConnection,
    runtime: tokio::runtime::Handle,
}

impl LedgerService {
    /// Build a service over an open `finance-db` connection. Must be
    /// called from inside a tokio runtime (the sync trait methods
    /// `block_on` the captured handle).
    ///
    /// # Errors
    /// Returns the static message if there is no current tokio runtime.
    pub fn new(db: DatabaseConnection) -> Result<Self, &'static str> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "LedgerService::new must be called from a tokio runtime")?;
        Ok(Self { db, runtime })
    }

    fn backend_err(e: sea_orm::DbErr) -> FinanceError {
        FinanceError::Backend {
            message: e.to_string(),
        }
    }

    async fn post_transaction_inner(&self, payload: PostTransaction) -> Result<Uuid, FinanceError> {
        // Enforce the double-entry invariant *before* any write:
        // splits must net to zero in the transaction's base currency.
        // We use `i128` for the running sum so a pathological set of
        // near-`i64::MAX` legs can't overflow the check itself.
        let off_by: i128 = payload
            .splits
            .0
            .iter()
            .map(|s| i128::from(s.fx_amount_minor))
            .sum();
        if off_by != 0 {
            // Saturate back to i64 for the error payload — the exact
            // magnitude past i64 range is irrelevant to the caller.
            let off_by_minor =
                i64::try_from(off_by).unwrap_or(if off_by > 0 { i64::MAX } else { i64::MIN });
            return Err(FinanceError::SplitsImbalanced { off_by_minor });
        }

        let id = Uuid::new_v4();
        let active = TransactionActive {
            id: Set(id),
            book_id: Set(payload.book_id),
            date: Set(payload.date),
            description: Set(payload.description),
            reference: Set(payload.reference),
            source_kind: Set(payload.source_kind),
            source_id: Set(payload.source_id),
            splits: Set(payload.splits),
            created_at: Set(Utc::now()),
        };
        active.insert(&self.db).await.map_err(Self::backend_err)?;
        Ok(id)
    }

    async fn account_transactions_inner(
        &self,
        account_id: Uuid,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<Transaction>, FinanceError> {
        // The posting date is a date-only ISO-8601 string; the
        // `since`/`until` window filters on `created_at` (the real
        // timestamp). Newest-first, capped by `limit`.
        let mut q = TransactionEntity::find().order_by_desc(TransactionColumn::CreatedAt);
        if let Some(since) = since {
            q = q.filter(TransactionColumn::CreatedAt.gte(since));
        }
        if let Some(until) = until {
            q = q.filter(TransactionColumn::CreatedAt.lt(until));
        }
        let cap = if limit == 0 {
            DEFAULT_TXN_LIMIT
        } else {
            u64::from(limit)
        };
        // Splits live inline as JSON, so there is no SQL-level join on
        // `account_id`. Pull the (windowed) rows and filter in memory
        // to those that touch the account, then honour the cap.
        let rows = q.all(&self.db).await.map_err(Self::backend_err)?;
        let out: Vec<Transaction> = rows
            .into_iter()
            .filter(|m| m.splits.0.iter().any(|s| s.account_id == account_id))
            .take(cap as usize)
            .map(Transaction::from)
            .collect();
        Ok(out)
    }

    async fn balances_inner(
        &self,
        book_id: Uuid,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Vec<AccountBalance>, FinanceError> {
        // Resolve the book's base currency once — accounts with an
        // empty `currency` inherit it.
        let base_currency = BookEntity::find_by_id(book_id)
            .one(&self.db)
            .await
            .map_err(Self::backend_err)?
            .map(|b| b.base_currency)
            .unwrap_or_default();

        let accounts = AccountEntity::find()
            .filter(AccountColumn::BookId.eq(book_id))
            .all(&self.db)
            .await
            .map_err(Self::backend_err)?;

        let mut txn_q = TransactionEntity::find().filter(TransactionColumn::BookId.eq(book_id));
        if let Some(as_of) = as_of {
            txn_q = txn_q.filter(TransactionColumn::CreatedAt.lte(as_of));
        }
        let txns = txn_q.all(&self.db).await.map_err(Self::backend_err)?;

        // Running per-account delta from splits, in each account's own
        // currency (`amount_minor`). Seed with opening balances.
        let mut deltas: std::collections::HashMap<Uuid, i128> = std::collections::HashMap::new();
        for t in &txns {
            for s in &t.splits.0 {
                *deltas.entry(s.account_id).or_default() += i128::from(s.amount_minor);
            }
        }

        let mut out = Vec::with_capacity(accounts.len());
        for a in accounts {
            let delta = deltas.get(&a.id).copied().unwrap_or(0);
            let total = i128::from(a.opening_balance_minor) + delta;
            let balance_minor =
                i64::try_from(total).unwrap_or(if total > 0 { i64::MAX } else { i64::MIN });
            let currency = if a.currency.is_empty() {
                base_currency.clone()
            } else {
                a.currency.clone()
            };
            out.push(AccountBalance {
                account_id: a.id,
                balance_minor,
                currency,
            });
        }
        Ok(out)
    }

    /// Async post — the same invariant-checked write the sync
    /// [`Ledger::post_transaction`] performs, but awaitable so callers
    /// already inside an async context (e.g. the invoicing backend's
    /// `block_on`) can post without nesting a `block_on`.
    ///
    /// # Errors
    /// [`FinanceError::SplitsImbalanced`] if the splits don't net to
    /// zero in base currency, or a backend error on write failure.
    pub async fn post(&self, payload: PostTransaction) -> Result<Uuid, FinanceError> {
        self.post_transaction_inner(payload).await
    }

    /// Find-or-create a canonical account by `(book, name, kind)`.
    /// Idempotent: looks up an existing account with the same name in
    /// the book first; creates one (inheriting the book's base
    /// currency, zero opening balance) only when absent. Returns the
    /// account id.
    ///
    /// Used by the invoicing flow to resolve its three standing
    /// accounts — "Accounts Receivable" (Asset), "Income" (Income),
    /// and "Cash" (Asset) — before posting.
    ///
    /// # Errors
    /// Backend error on lookup / insert failure.
    pub async fn ensure_account(
        &self,
        book_id: Uuid,
        name: &str,
        kind: AccountKind,
    ) -> Result<Uuid, FinanceError> {
        if let Some(m) = AccountEntity::find()
            .filter(AccountColumn::BookId.eq(book_id))
            .filter(AccountColumn::Name.eq(name))
            .one(&self.db)
            .await
            .map_err(Self::backend_err)?
        {
            return Ok(m.id);
        }
        let now = Utc::now();
        let acct = Account {
            id: Uuid::new_v4(),
            book_id,
            name: name.to_string(),
            kind,
            parent_id: Uuid::nil(),
            currency: String::new(), // inherit book base currency
            is_archived: false,
            opening_balance_minor: 0,
            notes: String::new(),
            created_at: now,
            updated_at: now,
        };
        let active = AccountActive {
            id: Set(acct.id),
            book_id: Set(acct.book_id),
            name: Set(acct.name),
            kind: Set(acct.kind),
            parent_id: Set(acct.parent_id),
            currency: Set(acct.currency),
            is_archived: Set(acct.is_archived),
            opening_balance_minor: Set(acct.opening_balance_minor),
            notes: Set(acct.notes),
            created_at: Set(acct.created_at),
            updated_at: Set(acct.updated_at),
        };
        active.insert(&self.db).await.map_err(Self::backend_err)?;
        Ok(acct.id)
    }

    async fn books_inner(&self) -> Result<Vec<Book>, FinanceError> {
        let rows = BookEntity::find()
            .all(&self.db)
            .await
            .map_err(Self::backend_err)?;
        Ok(rows.into_iter().map(book_from_model).collect())
    }

    async fn accounts_inner(&self, book_id: Uuid) -> Result<Vec<Account>, FinanceError> {
        let rows = AccountEntity::find()
            .filter(AccountColumn::BookId.eq(book_id))
            .order_by_asc(AccountColumn::Name)
            .all(&self.db)
            .await
            .map_err(Self::backend_err)?;
        Ok(rows.into_iter().map(account_from_model).collect())
    }
}

fn book_from_model(m: BookModel) -> Book {
    Book {
        id: m.id,
        name: m.name,
        kind: m.kind,
        base_currency: m.base_currency,
        settings_json: m.settings_json,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

fn account_from_model(m: AccountModel) -> Account {
    Account {
        id: m.id,
        book_id: m.book_id,
        name: m.name,
        kind: m.kind,
        parent_id: m.parent_id,
        currency: m.currency,
        is_archived: m.is_archived,
        opening_balance_minor: m.opening_balance_minor,
        notes: m.notes,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

impl Ledger for LedgerService {
    fn post_transaction(&self, payload: PostTransaction) -> Result<Uuid, FinanceError> {
        self.runtime.block_on(self.post_transaction_inner(payload))
    }

    fn account_transactions(
        &self,
        account_id: Uuid,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<Transaction>, FinanceError> {
        self.runtime
            .block_on(self.account_transactions_inner(account_id, since, until, limit))
    }

    fn balances(
        &self,
        book_id: Uuid,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Vec<AccountBalance>, FinanceError> {
        self.runtime.block_on(self.balances_inner(book_id, as_of))
    }

    fn books(&self) -> Result<Vec<Book>, FinanceError> {
        self.runtime.block_on(self.books_inner())
    }

    fn accounts(&self, book_id: Uuid) -> Result<Vec<Account>, FinanceError> {
        self.runtime.block_on(self.accounts_inner(book_id))
    }
}
