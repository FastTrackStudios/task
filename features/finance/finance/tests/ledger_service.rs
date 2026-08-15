//! Integration tests for the concrete [`finance::LedgerService`].
//!
//! Each test spins an in-memory SQLite, runs the `finance-db`
//! migrator, seeds a book + a couple of accounts, then exercises
//! the **synchronous** `Ledger` trait. Because the impl blocks on
//! its captured runtime handle, the sync calls must run off the
//! async worker — we drive them through `spawn_blocking`, exactly
//! as the real `#[architect::rpc]` dispatcher (`TokioBlockingDispatcher`)
//! does in production.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, Set};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

use finance::LedgerService;
use finance_db::Migrator;
use finance_db::entity::{AccountActive, BookActive};
use finance_proto::book::BookKind;
use finance_proto::error::FinanceError;
use finance_proto::ledger::{AccountKind, TransactionSource, TransactionSplit, TransactionSplits};
use finance_proto::service::ledger::{Ledger, PostTransaction};

struct Fixture {
    conn: DatabaseConnection,
    book_id: Uuid,
    cash: Uuid,
    income: Uuid,
}

async fn setup() -> Fixture {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    let now = Utc::now();

    let book_id = Uuid::new_v4();
    BookActive {
        id: Set(book_id),
        name: Set("Studio".into()),
        kind: Set(BookKind::Business),
        base_currency: Set("USD".into()),
        settings_json: Set("{}".into()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&conn)
    .await
    .unwrap();

    let cash = seed_account(&conn, book_id, "Cash", AccountKind::Asset, 0).await;
    let income = seed_account(&conn, book_id, "Sales", AccountKind::Income, 0).await;

    Fixture {
        conn,
        book_id,
        cash,
        income,
    }
}

async fn seed_account(
    conn: &DatabaseConnection,
    book_id: Uuid,
    name: &str,
    kind: AccountKind,
    opening_balance_minor: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now();
    AccountActive {
        id: Set(id),
        book_id: Set(book_id),
        name: Set(name.into()),
        kind: Set(kind),
        parent_id: Set(Uuid::nil()),
        currency: Set(String::new()), // inherit book base currency
        is_archived: Set(false),
        opening_balance_minor: Set(opening_balance_minor),
        notes: Set(String::new()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(conn)
    .await
    .unwrap();
    id
}

fn split(account_id: Uuid, amount_minor: i64) -> TransactionSplit {
    // Same-currency split: fx_amount == amount, rate == 1.0 (micro).
    TransactionSplit {
        account_id,
        amount_minor,
        fx_amount_minor: amount_minor,
        fx_rate_micro: 1_000_000,
        memo: String::new(),
    }
}

fn post(book_id: Uuid, splits: Vec<TransactionSplit>) -> PostTransaction {
    PostTransaction {
        book_id,
        date: "2026-05-28".into(),
        description: "test entry".into(),
        reference: String::new(),
        source_kind: TransactionSource::Manual,
        source_id: Uuid::nil(),
        splits: TransactionSplits(splits),
    }
}

/// Run a closure that uses the (sync) `Ledger` trait off the async
/// worker — the service blocks on its runtime handle internally, so
/// it must not be called on a runtime thread.
async fn on_blocking<T, F>(svc: LedgerService, f: F) -> T
where
    F: FnOnce(LedgerService) -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || f(svc))
        .await
        .expect("blocking task panicked")
}

#[tokio::test(flavor = "multi_thread")]
async fn balanced_transaction_posts_and_is_retrievable() {
    let fx = setup().await;
    let svc = LedgerService::new(fx.conn.clone()).unwrap();

    // Debit cash 100.00, credit income 100.00 → nets to zero.
    let payload = post(
        fx.book_id,
        vec![split(fx.cash, 10_000), split(fx.income, -10_000)],
    );
    let id = on_blocking(svc.clone(), move |s| s.post_transaction(payload))
        .await
        .expect("balanced transaction should post");

    // Retrievable via account history for both legs.
    let cash = fx.cash;
    let txns = on_blocking(svc, move |s| s.account_transactions(cash, None, None, 50))
        .await
        .expect("history query ok");
    assert_eq!(txns.len(), 1, "one transaction touches the cash account");
    assert_eq!(txns[0].id, id);
    assert_eq!(txns[0].splits.0.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn imbalanced_transaction_rejected_and_writes_nothing() {
    let fx = setup().await;
    let svc = LedgerService::new(fx.conn.clone()).unwrap();

    // Off by 500 minor units in the base currency.
    let payload = post(
        fx.book_id,
        vec![split(fx.cash, 10_000), split(fx.income, -9_500)],
    );
    let err = on_blocking(svc.clone(), move |s| s.post_transaction(payload))
        .await
        .expect_err("imbalanced transaction must be rejected");
    match err {
        FinanceError::SplitsImbalanced { off_by_minor } => assert_eq!(off_by_minor, 500),
        other => panic!("expected SplitsImbalanced, got {other:?}"),
    }

    // Nothing was written: history is empty and balances are still
    // the opening balances (zero).
    let cash = fx.cash;
    let txns = on_blocking(svc.clone(), move |s| {
        s.account_transactions(cash, None, None, 50)
    })
    .await
    .unwrap();
    assert!(txns.is_empty(), "rejected entry must not persist");

    let book_id = fx.book_id;
    let balances = on_blocking(svc, move |s| s.balances(book_id, None))
        .await
        .unwrap();
    assert!(
        balances.iter().all(|b| b.balance_minor == 0),
        "no balance should have moved"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn balances_reflect_posted_transactions() {
    let fx = setup().await;
    let svc = LedgerService::new(fx.conn.clone()).unwrap();

    // Two sales: 100.00 then 25.00 into cash, credited to income.
    for amt in [10_000_i64, 2_500] {
        let payload = post(
            fx.book_id,
            vec![split(fx.cash, amt), split(fx.income, -amt)],
        );
        on_blocking(svc.clone(), move |s| s.post_transaction(payload))
            .await
            .expect("balanced post ok");
    }

    let book_id = fx.book_id;
    let balances = on_blocking(svc, move |s| s.balances(book_id, None))
        .await
        .unwrap();

    let cash_bal = balances.iter().find(|b| b.account_id == fx.cash).unwrap();
    let income_bal = balances.iter().find(|b| b.account_id == fx.income).unwrap();
    assert_eq!(cash_bal.balance_minor, 12_500, "cash debited 125.00");
    assert_eq!(income_bal.balance_minor, -12_500, "income credited 125.00");
    // Currency inherited from the book's base currency.
    assert_eq!(cash_bal.currency, "USD");
    assert_eq!(income_bal.currency, "USD");
}

#[tokio::test(flavor = "multi_thread")]
async fn books_and_accounts_read_surface() {
    let fx = setup().await;
    let svc = LedgerService::new(fx.conn.clone()).unwrap();

    // books() returns the seeded book.
    let books = on_blocking(svc.clone(), |s| s.books())
        .await
        .expect("books ok");
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].id, fx.book_id);

    // accounts(book) returns the two seeded accounts, name-sorted.
    let book_id = fx.book_id;
    let accounts = on_blocking(svc, move |s| s.accounts(book_id))
        .await
        .expect("accounts ok");
    let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Cash", "Sales"], "name-sorted accounts");
}

#[tokio::test(flavor = "multi_thread")]
async fn ensure_account_is_idempotent() {
    let fx = setup().await;
    let svc = LedgerService::new(fx.conn.clone()).unwrap();
    let book_id = fx.book_id;

    // First call creates "Accounts Receivable"; second resolves it.
    // `ensure_account` is async (not a sync trait method) so it runs
    // directly on the test's runtime.
    let a1 = svc
        .ensure_account(book_id, "Accounts Receivable", AccountKind::Asset)
        .await
        .expect("create AR");
    let a2 = svc
        .ensure_account(book_id, "Accounts Receivable", AccountKind::Asset)
        .await
        .expect("resolve AR");
    assert_eq!(a1, a2, "ensure_account is find-or-create, not create-twice");

    // The book now has exactly one extra account (Cash + Sales + AR).
    // `accounts` is sync (blocks on the runtime), so drive it off the
    // worker via `spawn_blocking`.
    let accounts = on_blocking(svc, move |s| s.accounts(book_id))
        .await
        .expect("accounts ok");
    assert_eq!(accounts.len(), 3);
}
