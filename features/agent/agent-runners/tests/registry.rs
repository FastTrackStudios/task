//! Runner registry round-trip: register → list → heartbeat → stale
//! → deregister, against a real SQLite database.
//!
//! These drive the [`Backends`] trait rather than the inherent
//! methods wherever the trait covers it — that is the seam a runner
//! and the CLI actually use.

use agent_proto::backend::{AgentBackend, BackendKind};
use agent_proto::error::AgentError;
use agent_proto::runner::{Capability, RunnerProfile, RunnerScope, TicketRequirements};
use agent_proto::service::backends::Backends;
use agent_runners::{Migrator, STALE_AFTER, Store};
use chrono::{Duration as ChronoDuration, Utc};
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;

async fn store() -> Store {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    Store::new(conn)
}

fn runner(id: &str, caps: Vec<Capability>, max: u32) -> AgentBackend {
    AgentBackend {
        id: id.into(),
        label: id.into(),
        kind: BackendKind::CliBridge,
        config_json: String::new(),
        registered_at: Utc::now(),
        last_seen: None,
        runner: RunnerProfile {
            id: id.into(),
            capabilities: caps,
            scope: RunnerScope::unrestricted(),
            max_concurrent: max,
        },
    }
}

/// thebattleship: the workstation that may compile.
fn battleship() -> AgentBackend {
    runner(
        "thebattleship",
        vec![
            Capability::Records,
            Capability::Shell,
            Capability::Build,
            Capability::Repo("FastTrackStudios/FastTrackStudio".into()),
        ],
        4,
    )
}

/// The server-side runtime: reads source, never compiles.
fn hermes() -> AgentBackend {
    let mut b = runner(
        "hermes",
        vec![
            Capability::Records,
            Capability::Shell,
            Capability::Repo("FastTrackStudios/FastTrackStudio".into()),
        ],
        2,
    );
    b.kind = BackendKind::InProcess;
    b
}

#[tokio::test]
async fn a_runner_registers_and_appears_in_the_listing() {
    let s = store().await;
    s.upsert_backend(battleship()).await.unwrap();

    let listed = s.list_backends().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "thebattleship");
    assert_eq!(listed[0].runner.max_concurrent, 4);
    assert!(listed[0].runner.capabilities.contains(&Capability::Build));
}

#[tokio::test]
async fn registering_twice_updates_rather_than_duplicating() {
    let s = store().await;
    s.upsert_backend(battleship()).await.unwrap();

    let mut again = battleship();
    again.runner.max_concurrent = 8;
    s.upsert_backend(again).await.unwrap();

    let listed = s.list_backends().await.unwrap();
    assert_eq!(
        listed.len(),
        1,
        "a re-registering runner must not duplicate"
    );
    assert_eq!(listed[0].runner.max_concurrent, 8);
}

#[tokio::test]
async fn a_registration_survives_a_new_store_over_the_same_database() {
    // Stands in for a server restart: the connection is what the
    // process owns, the rows are what persists.
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    Store::new(conn.clone())
        .upsert_backend(battleship())
        .await
        .unwrap();

    let reopened = Store::new(conn);
    assert_eq!(reopened.list_backends().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_capability_outside_the_closed_vocabulary_is_refused() {
    let s = store().await;
    let mut bad = battleship();
    // `Repo("")` cannot be re-parsed from its own wire form, so it
    // is not in the vocabulary.
    bad.runner
        .capabilities
        .push(Capability::Repo(String::new()));

    let err = s.upsert_backend(bad).await.unwrap_err();
    assert!(
        matches!(err, AgentError::Invalid(_)),
        "expected Invalid, got {err:?}"
    );
    assert!(
        s.list_backends().await.unwrap().is_empty(),
        "a refused registration must not be stored"
    );
}

#[tokio::test]
async fn a_profile_id_that_disagrees_with_the_backend_id_is_refused() {
    let s = store().await;
    let mut mismatched = battleship();
    mismatched.runner.id = "somewhere-else".into();
    assert!(s.upsert_backend(mismatched).await.is_err());
}

#[tokio::test]
async fn a_runner_that_has_never_heartbeated_is_not_healthy() {
    let s = store().await;
    s.upsert_backend(battleship()).await.unwrap();

    let health = s.backend_health("thebattleship".into()).await.unwrap();
    assert!(!health.reachable);
    assert_eq!(health.state, "stale");
}

#[tokio::test]
async fn a_heartbeat_makes_a_runner_healthy_and_routable() {
    let s = store().await;
    s.upsert_backend(battleship()).await.unwrap();
    s.heartbeat_backend("thebattleship".into()).await.unwrap();

    let health = s.backend_health("thebattleship".into()).await.unwrap();
    assert!(health.reachable);
    assert_eq!(health.state, "running");
    assert_eq!(s.routable().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_lapsed_heartbeat_goes_stale_and_stops_being_offered_work() {
    let s = store().await;
    let mut b = battleship();
    b.last_seen = Some(
        Utc::now() - ChronoDuration::from_std(STALE_AFTER).unwrap() - ChronoDuration::seconds(1),
    );
    s.upsert_backend(b).await.unwrap();

    let health = s.backend_health("thebattleship".into()).await.unwrap();
    assert!(!health.reachable);
    assert!(health.status_text.contains("heartbeat"));

    assert!(
        s.routable().await.unwrap().is_empty(),
        "a stale runner keeps its registration but takes no work"
    );
    assert_eq!(
        s.list_backends().await.unwrap().len(),
        1,
        "going stale must not deregister it"
    );
}

#[tokio::test]
async fn heartbeating_an_unregistered_runner_says_so() {
    let s = store().await;
    let err = s.heartbeat_backend("ghost".into()).await.unwrap_err();
    assert!(matches!(err, AgentError::BackendNotFound(_)), "{err:?}");
}

#[tokio::test]
async fn deregistering_removes_it() {
    let s = store().await;
    s.upsert_backend(battleship()).await.unwrap();
    s.remove_backend("thebattleship".into()).await.unwrap();
    assert!(s.list_backends().await.unwrap().is_empty());
}

#[tokio::test]
async fn backends_filter_by_kind() {
    let s = store().await;
    s.upsert_backend(battleship()).await.unwrap();
    s.upsert_backend(hermes()).await.unwrap();

    let in_process = s.backends_by_kind(BackendKind::InProcess).await.unwrap();
    assert_eq!(in_process.len(), 1);
    assert_eq!(in_process[0].id, "hermes");
}

#[tokio::test]
async fn a_build_ticket_is_unroutable_when_only_the_server_runner_is_live() {
    // The end-to-end statement of the rule that keeps compilation
    // off the box serving the API.
    let s = store().await;
    s.upsert_backend(hermes()).await.unwrap();
    s.heartbeat_backend("hermes".into()).await.unwrap();

    let req = TicketRequirements {
        capabilities: vec![Capability::Build],
        org: "fasttrackstudios".into(),
        project: None,
    };
    assert_eq!(
        s.unroutable_reason(&req).await.unwrap(),
        Some("build".into())
    );

    // Bring the workstation up and the same ticket routes.
    s.upsert_backend(battleship()).await.unwrap();
    s.heartbeat_backend("thebattleship".into()).await.unwrap();
    assert_eq!(s.unroutable_reason(&req).await.unwrap(), None);
}

#[tokio::test]
async fn a_registered_but_stale_runner_does_not_make_work_routable() {
    let s = store().await;
    s.upsert_backend(battleship()).await.unwrap(); // never heartbeats

    let req = TicketRequirements {
        capabilities: vec![Capability::Build],
        org: "fasttrackstudios".into(),
        project: None,
    };
    assert_eq!(
        s.unroutable_reason(&req).await.unwrap(),
        Some("build".into()),
        "routing must consider live runners only"
    );
}
