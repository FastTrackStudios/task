//! End-to-end cover for the notifications pipeline: a task completed
//! over the mounted `TaskService` RPC → the notifier's task rule
//! fires → the notification arrives on the mounted `Notify` events
//! stream AND via `Notify::list`, and `mark_read` round-trips.
//!
//! Exercises the whole chain through one booted `AppState`:
//! dispatcher → task backend hub → notifier pump (in-process
//! LocalServer subscriber) → rule → InApp channel → notify store →
//! stream host → subscriber channel.
//!
//! Self-sandboxed: tempdir data root via `TASK_DATA_ROOT`, one test
//! per binary so the env setup races nothing.

use std::time::Duration;

use architect::Scope;
use notify_proto::{NotifyClient, NotifyEvent, NotifyKind, NotifyListFilter, NotifyStreamClient};
use task_server::AppState;

/// Receive the next event off a subscriber channel, cloned out of the
/// wire buffer, within `secs`.
async fn next_event<Ev: Clone + vox::facet::Facet<'static> + 'static>(
    rx: &mut vox::Rx<Ev>,
    secs: u64,
) -> Ev {
    let msg = tokio::time::timeout(Duration::from_secs(secs), rx.recv())
        .await
        .expect("event timeout")
        .expect("rx error")
        .expect("rx closed");
    let mut owned: Option<Ev> = None;
    let _ = msg.map(|ev| owned = Some(ev.clone()));
    owned.expect("decode event")
}

#[tokio::test(flavor = "multi_thread")]
async fn completing_a_task_notifies_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        for var in [
            "TASK_SERVER_ORG",
            "TASK_SERVER_VAULT_ROOT",
            "TASK_NOTIFY_WEBHOOK",
        ] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();

    // Boots the org (fresh empty data root → notify.sqlite migrated
    // from nothing) AND spawns the notifier.
    let state = AppState::new(None).await.expect("boot AppState");
    let scope = Scope::new();
    let local = state
        .local_server("alpha", &scope)
        .expect("alpha is hosted");

    // Subscribe to the notify stream before mutating anything.
    let stream: NotifyStreamClient = local.establish().await.expect("notify stream client");
    let (tx, mut rx) = vox::channel::<NotifyEvent>();
    let _sub = tokio::spawn(async move {
        let _ = stream.events(tx).await;
    });
    // Let the test's subscription AND the notifier's own task-stream
    // subscription attach before the first write.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Create an open task, then complete it over the mounted RPC.
    let tasks: task::TaskServiceClient = local.establish().await.expect("task client");
    let mut draft = task::capture("ship the notifier");
    draft.status = "open".into();
    let created = tasks.create(draft).await.expect("create task");
    let mut done = created.clone();
    done.status = "done".into();
    tasks.update(done).await.expect("complete task");

    // The completion arrives on the events stream…
    let notification = loop {
        match next_event(&mut rx, 10).await {
            NotifyEvent::Upserted(n) if n.kind == NotifyKind::TaskCompleted => break n,
            // Other rule hits (none expected here) or read-flips are
            // skipped rather than failing the test.
            _ => {}
        }
    };
    assert_eq!(notification.title, "Task done: ship the notifier");
    assert_eq!(notification.source.service, "task");
    assert_eq!(notification.source.entity, created.id.to_string());
    assert_eq!(notification.source.href, format!("/tasks/{}", created.id));
    assert!(notification.read_at.is_none());

    // …and via list().
    let notify: NotifyClient = local.establish().await.expect("notify client");
    let unread = notify
        .list(NotifyListFilter {
            unread_only: true,
            ..Default::default()
        })
        .await
        .expect("list unread");
    assert!(
        unread.iter().any(|n| n.id == notification.id),
        "the completion notification is listed unread"
    );

    // mark_read round-trips: the stream folds the flip and the unread
    // view drops the row.
    let read = notify.mark_read(notification.id).await.expect("mark_read");
    assert!(read.read_at.is_some());
    loop {
        match next_event(&mut rx, 10).await {
            NotifyEvent::Upserted(n) if n.id == notification.id && n.read_at.is_some() => break,
            _ => {}
        }
    }
    let unread = notify
        .list(NotifyListFilter {
            unread_only: true,
            ..Default::default()
        })
        .await
        .expect("list unread again");
    assert!(unread.iter().all(|n| n.id != notification.id));

    scope.close().await;
    state.scope.close().await;
}
