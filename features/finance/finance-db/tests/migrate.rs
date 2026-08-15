//! Smoke test — migrations run cleanly against an in-memory
//! SQLite. Confirms the per-table column lists in
//! `migrations.rs` line up with the SeaORM model emission
//! from `finance-proto`.

use finance_db::Migrator;
use finance_db::entity::{BookActive, BookEntity, BookModel, PartyActive, PartyEntity};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

#[tokio::test]
async fn migrations_run_clean() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
}

#[tokio::test]
async fn round_trip_book_via_architect_emission() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();

    // Insert a book through the architect-emitted ActiveModel.
    let book_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    BookActive {
        id: Set(book_id),
        name: Set("Personal".into()),
        kind: Set(finance_proto::book::BookKind::Personal),
        base_currency: Set("USD".into()),
        settings_json: Set("{}".into()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&conn)
    .await
    .unwrap();

    // Read it back.
    let loaded: BookModel = BookEntity::find_by_id(book_id)
        .one(&conn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.name, "Personal");
    assert_eq!(loaded.base_currency, "USD");
}

#[tokio::test]
async fn parties_cascade_with_book_delete() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    // SQLite needs PRAGMA for FK enforcement at the
    // connection level; the migrator doesn't set it.
    sea_orm::ConnectionTrait::execute_unprepared(&conn, "PRAGMA foreign_keys = ON;")
        .await
        .unwrap();

    let book_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    BookActive {
        id: Set(book_id),
        name: Set("Studio".into()),
        kind: Set(finance_proto::book::BookKind::Business),
        base_currency: Set("USD".into()),
        settings_json: Set("{}".into()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&conn)
    .await
    .unwrap();

    let party_id = Uuid::new_v4();
    PartyActive {
        id: Set(party_id),
        book_id: Set(book_id),
        kind: Set(finance_proto::party::PartyKind::Client),
        display_name: Set("ACME".into()),
        legal_name: Set("ACME Corp".into()),
        email: Set("billing@acme.test".into()),
        phone: Set(String::new()),
        address: Set(String::new()),
        tax_id: Set(String::new()),
        default_currency: Set("USD".into()),
        default_net_days: Set(30),
        default_rate_minor_per_hour: Set(15000),
        notes: Set(String::new()),
        is_archived: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&conn)
    .await
    .unwrap();

    // Delete the book → party FK cascades.
    BookEntity::delete_by_id(book_id).exec(&conn).await.unwrap();
    let remaining = PartyEntity::find_by_id(party_id).one(&conn).await.unwrap();
    assert!(
        remaining.is_none(),
        "party should cascade-delete with its book"
    );
}
