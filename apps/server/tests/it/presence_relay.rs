//! Presence relay over the same mount `org_layer_router` uses — two
//! peers on one org's host must see each other. Guards the wiring this
//! repo owns (host construction + dispatcher mount + the ui's entry
//! encoding assumptions); the relay protocol itself is covered by
//! architect's e2e suite.

use std::time::Duration;

use architect::{LocalServer, Scope};
use task_server::presence::{PRESENCE_DOC_ID, PRESENCE_TIMEOUT_MS};

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn presence_relays_between_two_peers() {
    // Exactly the org_layer_router mount: one shared host, one
    // dispatcher pair, served in-process.
    let host = crdt::sync::PresenceHost::new(PRESENCE_DOC_ID, PRESENCE_TIMEOUT_MS);
    let router = architect::LayerRouter::new().with(
        crdt::sync::doc_presence_service_descriptor(),
        crdt::sync::DocPresenceDispatcher::new(host),
    );
    let scope = Scope::new();
    let local = LocalServer::serve(router, scope.clone());

    let (peer_a, mut driver_a) =
        crdt::sync::PresencePeer::new(PRESENCE_DOC_ID, PRESENCE_TIMEOUT_MS);
    let client_a: crdt::sync::DocPresenceClient = local.establish().await.expect("establish a");
    tokio::spawn(async move { driver_a.run(&client_a).await });

    let (peer_b, mut driver_b) =
        crdt::sync::PresencePeer::new(PRESENCE_DOC_ID, PRESENCE_TIMEOUT_MS);
    let client_b: crdt::sync::DocPresenceClient = local.establish().await.expect("establish b");
    tokio::spawn(async move { driver_b.run(&client_b).await });

    peer_a.set("client-a", "alice");
    let pb = peer_b.clone();
    eventually("b sees a's presence", move || {
        matches!(pb.states().get("client-a"), Some(v) if format!("{v:?}").contains("alice"))
    })
    .await;

    peer_b.set("client-b", "bob");
    let pa = peer_a.clone();
    eventually(
        "a sees b's presence",
        move || matches!(pa.states().get("client-b"), Some(v) if format!("{v:?}").contains("bob")),
    )
    .await;
}
