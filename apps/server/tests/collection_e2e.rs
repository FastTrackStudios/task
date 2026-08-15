// This test exercises services owned by the `fasttrackstudio` plugin;
// a build without it has nothing to cover.
#![cfg(feature = "plugin-fasttrackstudio")]
#![allow(clippy::large_futures)]
//! End-to-end check for the `CollectionService` architect-rpc service
//! against a live `task-server`. Boots `AppState` on an ephemeral TCP
//! port (with `TASK_SERVER_COLLECTIONS_PATH` — and the vault root —
//! pointed at a temp dir so the real org data root is never touched),
//! connects a `CollectionServiceClient`, and drives the core mutation
//! path over vox: `create` → `add_item` (append + insert-after) →
//! `list` → `reorder`, asserting the round-trip order each step.
//!
//! Proves the dispatcher mounted in `org_layer_router` is reachable
//! over the WebSocket transport and that the JSONL store round-trips
//! through it. The architect macro rewrites the trait's `&str` args
//! into owned `String` on the async client side, so call sites here
//! pass owned strings.

use collection_proto::{CollectionKind, CollectionServiceClient, NodeRef, Placement};

/// Serializes env-var twiddling across the async test pool — the same
/// guard `vault_sync_e2e` uses, so the two suites don't race on the
/// shared process env while `AppState::new` reads it.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn boot_server() -> eyre::Result<(String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` for the duration of `AppState::new`,
    // which reads both vars exactly once (captured into `OrgAppState`).
    unsafe {
        // Sandbox the DATA root too, not just the vault root: without
        // it `DataRoot::from_env` falls back to `$HOME/.task` — the
        // developer's (and on this fleet, production's) real data root —
        // and the server writes its storage registry, in-server agent
        // identity and volume directory straight into it (PR #284
        // review).
        std::env::set_var("TASK_DATA_ROOT", tmp.path().join("data"));
        std::env::set_var("TASK_SERVER_VAULT_ROOT", tmp.path());
        std::env::set_var(
            "TASK_SERVER_COLLECTIONS_PATH",
            tmp.path().join("collections.jsonl"),
        );
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .ensure()
        .map_err(|e| eyre::eyre!("ensure data root: {e}"))?;
    data_root
        .init_org("home", "Home", true)
        .map_err(|e| eyre::eyre!("scaffold home org: {e}"))?;
    let state = task_server::AppState::new(None).await?;
    drop(guard);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = format!("ws://127.0.0.1:{port}/vox");
    Ok((url, tmp))
}

async fn connect(url: &str) -> eyre::Result<CollectionServiceClient> {
    vox::connect_lane(url)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("collection connect: {e:?}"))
}

fn ids(c: &collection_proto::Collection) -> Vec<String> {
    c.items.iter().map(|it| it.node.id.clone()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn create_add_list_reorder_round_trip() {
    let (url, _tmp) = boot_server().await.unwrap();
    let client = connect(&url).await.unwrap();

    // create — the backend assigns the id.
    let set = client
        .create(
            "acme".to_string(),
            "Sunday Set".to_string(),
            CollectionKind::Setlist,
        )
        .await
        .expect("create round-trips over vox");
    assert!(!set.id.is_empty(), "create should assign an id");

    // add_item — append three songs, then insert one after "a".
    for slug in ["a", "b", "c"] {
        client
            .add_item(Placement {
                collection_id: set.id.clone(),
                node: NodeRef::song(slug),
                after: None,
            })
            .await
            .expect("add_item append round-trips");
    }
    let after_insert = client
        .add_item(Placement {
            collection_id: set.id.clone(),
            node: NodeRef::song("x"),
            after: Some(NodeRef::song("a")),
        })
        .await
        .expect("add_item insert-after round-trips");
    assert_eq!(ids(&after_insert), ["a", "x", "b", "c"]);

    // list — the setlist is visible in the org, filtered by kind.
    let listed = client
        .list("acme".to_string(), Some(CollectionKind::Setlist))
        .await
        .expect("list round-trips");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, set.id);
    // A different kind filter excludes it.
    assert!(
        client
            .list("acme".to_string(), Some(CollectionKind::Library))
            .await
            .expect("list(library) round-trips")
            .is_empty()
    );

    // reorder — move "c" to right after "a"; only that rank changes.
    let reordered = client
        .reorder(Placement {
            collection_id: set.id.clone(),
            node: NodeRef::song("c"),
            after: Some(NodeRef::song("a")),
        })
        .await
        .expect("reorder round-trips");
    assert_eq!(ids(&reordered), ["a", "c", "x", "b"]);

    // get — the persisted collection reflects the final order.
    let fetched = client
        .get(set.id.clone())
        .await
        .expect("get round-trips")
        .expect("collection exists");
    assert_eq!(ids(&fetched), ["a", "c", "x", "b"]);
}
