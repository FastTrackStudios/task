//! `offline.peers` end to end: a task file written by anything but the
//! task service — here a plain vault `put_file`, standing in for a
//! document merged from a peer, the desktop folder, or an agent writing
//! markdown — is announced on the task stream, and so is its removal.
//! Every list, the activity feed and the notifier ride that stream, so
//! this is what makes a task synced from elsewhere show up live.

use std::time::Duration;

use architect::Scope;
use task::TaskServiceStreamClient;
use task::service::TaskEvent;
use task_server::AppState;
use vault_proto::{IfMatch, VaultSyncClient};

async fn next_event(rx: &mut vox::Rx<TaskEvent>, secs: u64) -> TaskEvent {
    let msg = tokio::time::timeout(Duration::from_secs(secs), rx.recv())
        .await
        .expect("event timeout")
        .expect("rx error")
        .expect("rx closed");
    let mut owned: Option<TaskEvent> = None;
    let _ = msg.map(|ev| owned = Some(ev.clone()));
    owned.expect("decode event")
}

// t[verify offline.peers] — a task file from outside reaches the task stream
#[tokio::test(flavor = "multi_thread")]
async fn a_task_file_written_outside_the_task_service_is_announced() {
    let tmp = tempfile::tempdir().unwrap();
    let env_guard = crate::support::env_lock().await;
    // SAFETY: serialised by the env lock above.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        for var in ["TASK_SERVER_ORG", "TASK_SERVER_VAULT_ROOT"] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();

    let state = AppState::new(None).await.expect("boot AppState");
    drop(env_guard);
    let scope = Scope::new();
    let local = state
        .local_server("alpha", &scope)
        .expect("alpha is hosted");

    let stream: TaskServiceStreamClient = local.establish().await.expect("task stream");
    let (tx, mut rx) = vox::channel::<TaskEvent>();
    let _sub = tokio::spawn(async move {
        let _ = stream.events(tx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The file arrives as bytes over the vault, not as a task call.
    let mut t = task::capture("Restring the bass @studio");
    t.path = task::doc::default_task_path(&t.title, None);
    let raw = task::doc::serialize_task(&t).expect("serialize");
    let vault: VaultSyncClient = local.establish().await.expect("vault client");
    vault
        .put_file(
            "default".into(),
            t.path.clone(),
            raw.into_bytes(),
            IfMatch::CreateOnly,
        )
        .await
        .expect("put the task file");

    match next_event(&mut rx, 10).await {
        TaskEvent::Upserted(got) => {
            assert_eq!(got.id, t.id);
            assert_eq!(got.title, t.title);
            assert_eq!(got.path, t.path);
        }
        other => panic!("expected the task, got {other:?}"),
    }

    vault
        .delete_file("default".into(), t.path.clone(), IfMatch::Force)
        .await
        .expect("delete the task file");
    match next_event(&mut rx, 10).await {
        TaskEvent::Deleted(id) => assert_eq!(id, t.id),
        other => panic!("expected the removal, got {other:?}"),
    }
}
