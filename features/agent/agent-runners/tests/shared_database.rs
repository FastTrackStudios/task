//! Why the registry owns its own SQLite file.
//!
//! Two SeaORM migrators over one database share a single
//! `seaql_migrations` table, and the second one to run **silently
//! applies nothing** — no error, no table. Colocating the registry
//! with the agent-task queue cost an afternoon exactly once; these
//! tests make sure it costs nobody a second one.

use agent_proto::backend::{AgentBackend, BackendKind};
use agent_proto::runner::{Capability, RunnerProfile, RunnerScope};
use agent_proto::service::backends::Backends;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;

fn runner(id: &str) -> AgentBackend {
    AgentBackend {
        id: id.into(),
        label: id.into(),
        kind: BackendKind::CliBridge,
        config_json: String::new(),
        registered_at: chrono::Utc::now(),
        last_seen: None,
        runner: RunnerProfile {
            id: id.into(),
            capabilities: vec![Capability::Records, Capability::Build],
            scope: RunnerScope::unrestricted(),
            max_concurrent: 2,
        },
    }
}

async fn has_table(conn: &DatabaseConnection, name: &str) -> bool {
    conn.query_all(Statement::from_string(
        conn.get_database_backend(),
        format!("SELECT name FROM sqlite_master WHERE type='table' AND name='{name}'"),
    ))
    .await
    .unwrap()
    .len()
        == 1
}

/// The arrangement the server uses: the registry has its own
/// database, so its migrator owns its own `seaql_migrations`.
#[tokio::test]
async fn the_registry_migrates_cleanly_in_its_own_database() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    agent_runners::Migrator::up(&conn, None).await.unwrap();
    assert!(has_table(&conn, "agent_backends").await);

    let store = agent_runners::Store::new(conn);
    store.upsert_backend(runner("thebattleship")).await.unwrap();
    assert_eq!(store.list_backends().await.unwrap().len(), 1);
}

/// Re-running the migrator is a no-op, as it is on every boot.
#[tokio::test]
async fn re_running_the_migrator_is_idempotent() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    for _ in 0..3 {
        agent_runners::Migrator::up(&conn, None).await.unwrap();
    }
    let store = agent_runners::Store::new(conn);
    store.upsert_backend(runner("thebattleship")).await.unwrap();
    assert_eq!(store.list_backends().await.unwrap().len(), 1);
}

/// Two migrators must not share a database — pinned so the reason is
/// documented rather than rediscovered.
///
/// SeaORM keeps one `seaql_migrations` table per database, and a
/// migrator refuses to run against applied migrations it does not
/// own. Sharing a file therefore breaks at boot.
///
/// **The trap that cost an afternoon:** `#[derive(DeriveMigrationName)]`
/// names a migration after the *file* it lives in, not the module. In
/// a file called `migrations.rs` every migration is therefore named
/// `migrations` — so two crates' migrations collide on one name, and
/// two migrations in one crate collide with each other. Both
/// migrators here declare their names explicitly for that reason;
/// the error text below still shows the old derived name arriving
/// from `agent-tasks`, which has not been converted.
#[tokio::test]
async fn two_migrators_over_one_database_refuse_to_run() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();

    agent_tasks::Migrator::up(&conn, None).await.unwrap();
    let second = agent_runners::Migrator::up(&conn, None).await;

    let err = second.expect_err("sharing a database must not quietly succeed");
    assert!(
        err.to_string().contains("has been applied"),
        "expected a complaint about foreign applied migrations, got: {err}"
    );
    assert!(
        !has_table(&conn, "agent_backends").await,
        "nothing of ours should have been created"
    );
}

/// Every migration in one migrator gets its own name.
///
/// Directly guards the collision above: two migrations sharing a
/// name fail with a UNIQUE violation on `seaql_migrations.version`,
/// which is what happens if someone adds a third migration with
/// `DeriveMigrationName` in this file.
#[tokio::test]
async fn each_migration_has_a_distinct_name() {
    let migrations = agent_runners::Migrator::migrations();
    let names: Vec<&str> = migrations.iter().map(|m| m.name()).collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        names.len(),
        unique.len(),
        "duplicate migration names: {names:?}"
    );
    assert!(
        !names.contains(&"migrations"),
        "`migrations` is the file-derived name — declare names explicitly: {names:?}"
    );
}
