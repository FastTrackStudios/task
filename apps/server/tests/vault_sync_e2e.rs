#![allow(clippy::large_futures)]
//! End-to-end check for the `VaultSync` architect-rpc service
//! against a live `task-server`. Boots `AppState` on an
//! ephemeral TCP port over the repo's example studio (see
//! `support`), connects a `VaultSyncClient`, and exercises
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

use vault_proto::{IfMatch, VaultChange, VaultEvent, VaultSyncClient, VaultSyncError};
use vox::VoxError;

// Each binary uses a slice of the shared boot helpers; "unused"
// here means "this binary did not need that one".
#[allow(dead_code)]
mod support;

/// Boot over the example studio — see `support`. The vault the tests
/// write into already holds [`support::EXAMPLE_PAGE`], which is the
/// point: an e2e that only ever meets pages it wrote itself proves less
/// than one that shares the vault with existing content.
async fn boot_server() -> eyre::Result<(String, tempfile::TempDir)> {
    support::boot_ws().await
}

async fn connect(url: &str) -> eyre::Result<VaultSyncClient> {
    vox::connect_lane(url)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("vault-sync connect: {e:?}"))
}

/// The next event on one vault id, skipping every other shelf's.
///
/// Bounded rather than a loop with no floor: a stream that never
/// produces the id being waited for should fail with "no event on
/// `default`" rather than hang until the harness kills it.
///
/// A macro rather than a function because the receiver's type is
/// `vox::channel`'s and naming it here would pin this test to a wire
/// detail it has no opinion about.
macro_rules! next_on {
    ($rx:expr, $vault_id:expr) => {{
        let mut found = None;
        for _ in 0..64 {
            let msg = tokio::time::timeout(Duration::from_secs(2), $rx.recv())
                .await
                .expect("event timeout")
                .expect("rx error")
                .expect("rx closed");
            let change = msg.get();
            if change.vault_id == $vault_id {
                found = Some(change.event.clone());
                break;
            }
        }
        found.unwrap_or_else(|| panic!("no event on `{}` in 64 messages", $vault_id))
    }};
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
    // The page we put, and the example vault's own — both present.
    //
    // Membership rather than an exact list: the seeded vault is allowed
    // to grow, and pinning the whole listing meant adding one example
    // file failed this test with a message about manifests. What
    // sandboxing actually needs is below.
    let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"notes/a.md"), "{paths:?}");
    assert!(paths.contains(&support::EXAMPLE_PAGE), "{paths:?}");
    // The sandbox proof, stated directly: every path is relative and
    // inside this vault. A boot that leaked another org's vault — or the
    // developer's own — shows up here as an absolute path or a climb.
    for p in &paths {
        assert!(
            !p.starts_with('/') && !p.contains(".."),
            "manifest path escapes the vault: {p}"
        );
    }
    let put = manifest
        .files
        .iter()
        .find(|f| f.path == "notes/a.md")
        .expect("the put page is in the manifest");
    assert_eq!(put.size, 5);

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

    // The stream is one channel over EVERY shelf the org holds — its
    // vault, its wikis, its asset groups and (since ADR 0004's fourth
    // root) each of its projects — which is why the event carries a
    // `vault_id` at all. So the assertion has to name the vault it
    // asked about rather than trusting the next message to be its own.
    //
    // It used to trust it, and got away with it because nothing else on
    // the seeded disk was writing. A project shelf's `project.md`
    // landing between the put and the delete is what turned "the next
    // message" into a coin toss, and a test that reads another shelf's
    // event as its own would go on failing intermittently forever.
    let change = next_on!(rx, "default");
    match &change {
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
    match &next_on!(rx, "default") {
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
