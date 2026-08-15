//! Round-trip test for the identity-locker store: tokens are stored
//! encrypted, come back decrypted, and upsert is keyed on
//! `(home_user_id, remote_url, remote_slug)`.

use identity::store::LinkRecord;
use identity::{Migrator, Store};
use sea_orm::{ConnectionTrait, Database, Statement};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

const SECRET: &str = "a-test-secret-at-least-32-bytes!!";

fn new_link(home_user_id: Uuid, token: Option<&str>) -> LinkRecord {
    LinkRecord {
        id: Uuid::nil(),
        home_user_id,
        label: "My Other Server".into(),
        remote_url: "https://remote.example".into(),
        remote_slug: "acme".into(),
        remote_user_id: Some(Uuid::new_v4()),
        remote_email: Some("me@remote.example".into()),
        token: token.map(str::to_string),
        expires_at: Some(1_800_000_000),
    }
}

#[tokio::test]
async fn token_round_trips_encrypted() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    Migrator::up(&conn, None).await.expect("run migrations");

    let store = Store::new(conn.clone(), SECRET.into());
    let home_user_id = Uuid::new_v4();

    let stored = store
        .upsert_link(new_link(home_user_id, Some("remote-token-xyz")))
        .await
        .expect("upsert link");
    assert_eq!(stored.token.as_deref(), Some("remote-token-xyz"));
    assert!(!stored.id.is_nil(), "insert mints a fresh id");

    // list_links decrypts back to the plaintext token.
    let links = store.list_links(home_user_id).await.expect("list links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].token.as_deref(), Some("remote-token-xyz"));

    // The raw stored ciphertext is NOT the plaintext.
    let raw = conn
        .query_one(Statement::from_string(
            conn.get_database_backend(),
            "SELECT token_ciphertext FROM linked_servers",
        ))
        .await
        .expect("query raw row")
        .expect("one row");
    let ciphertext: String = raw.try_get("", "token_ciphertext").expect("get ciphertext");
    assert_ne!(ciphertext, "remote-token-xyz");
    assert!(!ciphertext.contains("remote-token-xyz"));
    assert!(ciphertext.starts_with("v2."), "AEAD envelope shape");

    // Upserting the same (home_user_id, remote_url, remote_slug)
    // updates in place — no duplicate row.
    let updated = store
        .upsert_link(new_link(home_user_id, Some("rotated-token")))
        .await
        .expect("upsert same key");
    assert_eq!(updated.id, stored.id, "same row id");
    assert_eq!(updated.token.as_deref(), Some("rotated-token"));

    let links = store.list_links(home_user_id).await.expect("list links");
    assert_eq!(links.len(), 1, "no duplicate row");
    assert_eq!(links[0].token.as_deref(), Some("rotated-token"));
}
