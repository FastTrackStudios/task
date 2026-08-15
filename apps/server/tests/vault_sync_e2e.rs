#![allow(clippy::large_futures)]
//! End-to-end check for the `VaultSync` architect-rpc service
//! against a live `task-server`. Boots `AppState` on an
//! ephemeral TCP port (with `TASK_SERVER_VAULT_ROOT` pointed at
//! a temp dir), connects a `VaultSyncClient`, and exercises
//! PUT → manifest → GET, the `#[subscribe] changes` stream
//! observing PUT + DELETE, and a conflict round-trip.
//!
//! The server registers exactly one vault per org under the id
//! `"default"` (`vault::Backend::single` — commit 03d6a09 moved
//! away from `under_parent`, whose ghost `default/` subdir broke
//! every vault-walking backend; this test tracked that change).
//! The architect macro rewrites the sync trait's `&str` args
//! into owned `String` on the async client side, so call sites
//! here pass owned strings.

use std::time::Duration;

use task_server::{AppState, router};
use vault_proto::{IfMatch, VaultChange, VaultEvent, VaultSyncClient, VaultSyncError};
use vox::VoxError;

/// Serializes env-var twiddling. `cargo test` runs tests on a
/// shared thread pool; without this, two `boot_server` calls
/// would race on `TASK_SERVER_VAULT_ROOT` and one could read
/// the other test's path. The mutex is held only across
/// `AppState::new` — once the value is captured into
/// `state.vault_sync`, subsequent env mutations are irrelevant.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn boot_server() -> eyre::Result<(String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` for the duration of
    // `AppState::new`, which reads the var exactly once.
    unsafe {
        std::env::set_var("TASK_SERVER_VAULT_ROOT", tmp.path());
    }
    let state = AppState::new(None).await?;
    drop(guard);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = format!("ws://127.0.0.1:{port}/vox");
    Ok((url, tmp))
}

async fn connect(url: &str) -> eyre::Result<VaultSyncClient> {
    vox::connect_lane(url)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("vault-sync connect: {e:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn put_manifest_get_round_trip() {
    let (url, _tmp) = boot_server().await.unwrap();
    let client = connect(&url).await.unwrap();

    let ack = client
        .put_file(
            "default".to_string(),
            "notes/a.md".to_string(),
            b"hello".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    assert!(!ack.sha256.is_empty(), "PUT should return a sha");

    let manifest = client.manifest("default".to_string()).await.unwrap();
    assert_eq!(manifest.vault_id, "default");
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].path, "notes/a.md");
    assert_eq!(manifest.files[0].size, 5);

    let bytes = client
        .get_file("default".to_string(), "notes/a.md".to_string())
        .await
        .unwrap();
    assert_eq!(&bytes.0[..], b"hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribe_receives_put_and_delete() {
    let (url, _tmp) = boot_server().await.unwrap();
    let client: vault_proto::VaultSyncStreamClient = vox::connect_lane(&url)
        .establish()
        .await
        .expect("vault-sync stream connect");
    let writer = connect(&url).await.unwrap();

    let (tx, mut rx) = vox::channel::<VaultChange>();
    let _sub = tokio::spawn(async move {
        let _ = client.changes(tx).await;
    });

    // Tiny delay so the subscribe handler is fully attached
    // before we emit the event.
    tokio::time::sleep(Duration::from_millis(50)).await;

    writer
        .put_file(
            "default".to_string(),
            "a.md".to_string(),
            b"x".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event timeout")
        .expect("rx error")
        .expect("rx closed");
    let change = msg.get();
    assert_eq!(change.vault_id, "default", "event carries its vault id");
    match &change.event {
        VaultEvent::Put { path, size, .. } => {
            assert_eq!(path, "a.md");
            assert_eq!(*size, 1);
        }
        other => panic!("expected Put, got {other:?}"),
    }

    writer
        .delete_file("default".to_string(), "a.md".to_string(), IfMatch::Force)
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event timeout")
        .expect("rx error")
        .expect("rx closed");
    match &msg.get().event {
        VaultEvent::Delete { path } => assert_eq!(path, "a.md"),
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn put_conflict_returns_server_bytes() {
    let (url, _tmp) = boot_server().await.unwrap();
    let client = connect(&url).await.unwrap();
    client
        .put_file(
            "default".to_string(),
            "x.md".to_string(),
            b"first".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let err = client
        .put_file(
            "default".to_string(),
            "x.md".to_string(),
            b"second".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap_err();
    match err {
        VoxError::User(boxed) => match *boxed {
            VaultSyncError::Conflict {
                server_sha,
                server_bytes,
            } => {
                assert!(!server_sha.is_empty());
                assert_eq!(&server_bytes[..], b"first");
            }
            other => panic!("expected Conflict, got {other:?}"),
        },
        other => panic!("expected User(Conflict), got {other:?}"),
    }
}
