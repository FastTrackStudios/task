//! `#[subscribe] fn events` end to end over the in-process transport.
//!
//! Serves a tempdir-backed [`task::TaskBackend`] through
//! `architect::LocalServer` (the same `LayerRouter` shape
//! `task-server` mounts: base `TaskService` + its stream sibling),
//! subscribes a `TaskServiceStreamClient`, then mutates through the
//! `TaskServiceClient` and asserts the publish-on-write events
//! arrive: create ⇒ `Upserted(full TaskInfo)`, delete ⇒ `Deleted(id)`.
//!
//! **The subscribe call is held in flight, not awaited.** vox scopes
//! channels to their request, so the generated stream host attaches
//! the sink and then parks in `pending()` — the call staying pending
//! IS the healthy subscribed state, and it resolves only when the
//! subscription ends (see `architect::use_stream`). Awaiting
//! `events(tx)` therefore hangs forever; the test drives it as a
//! background task and waits for the sink to reach the backend hub
//! (`PubSub::subscriber_count`) before mutating, so no event is missed.

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use task::service::TaskEvent;
use task::{TaskBackend, TaskServiceClient, TaskServiceStreamClient, TaskServiceStreamSource};

/// Receive the next event, copying it out of the `SelfRef` lend
/// (same pattern as `architect::use_stream`). Bounded so a missing
/// publish fails the test instead of hanging it.
async fn next_event(rx: &mut vox::Rx<TaskEvent>) -> TaskEvent {
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for a TaskEvent")
        .expect("event channel errored")
        .expect("event stream closed early");
    let mut copied = None;
    let _ = frame.map(|ev| copied = Some(ev));
    copied.expect("SelfRef::map ran")
}

#[tokio::test(flavor = "multi_thread")]
async fn create_and_delete_reach_subscriber() {
    let vault = tempfile::tempdir().expect("tempdir");
    let backend = TaskBackend::new(vault.path());

    // Base service + stream sibling on one router — what
    // `task-server`'s `org_layer_router` does for the org backend.
    let router = LayerRouter::new()
        .merge(task::task_service_layer(backend.clone()))
        .merge(task::task_service_stream_layer(backend.clone()));

    let scope = Scope::new();
    let local = LocalServer::serve(router, scope.clone());

    let tasks: TaskServiceClient = local
        .establish()
        .await
        .expect("establish TaskServiceClient");
    let stream: TaskServiceStreamClient = local
        .establish()
        .await
        .expect("establish TaskServiceStreamClient");

    // Subscribe BEFORE mutating. The call stays in flight for the
    // life of the subscription (see the module docs), so spawn it
    // and then wait for the sink to attach to the backend hub —
    // once `subscriber_count() > 0` the publish-on-write path
    // reaches us and nothing is missed.
    let (tx, mut rx) = vox::channel::<TaskEvent>();
    let subscription = tokio::spawn(async move {
        stream.events(tx).await.expect("subscribe to task events");
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while backend.events_hub().subscriber_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("subscriber sink never reached the backend hub");

    // create ⇒ Upserted carrying the full post-write task.
    let created = tasks
        .create(task::capture("Live board event"))
        .await
        .expect("create over local transport");
    match next_event(&mut rx).await {
        TaskEvent::Upserted(t) => {
            assert_eq!(t.id, created.id, "Upserted carries the created id");
            assert_eq!(t.title, "Live board event");
            assert_eq!(t.path, created.path, "payload is the post-write state");
        }
        other @ TaskEvent::Deleted(_) => {
            panic!("expected Upserted after create, got {other:?}")
        }
    }

    // delete ⇒ Deleted carrying the id.
    tasks
        .delete(created.id)
        .await
        .expect("delete over local transport");
    assert_eq!(
        next_event(&mut rx).await,
        TaskEvent::Deleted(created.id),
        "expected Deleted after delete"
    );

    // End the subscription (the in-flight call never resolves on its
    // own — dropping/aborting it is the unsubscribe), then tear down.
    subscription.abort();
    scope.close().await;
}
