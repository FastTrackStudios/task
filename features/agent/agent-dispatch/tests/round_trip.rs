//! End-to-end dispatch → claim → complete round-trip.
//! Exercises both sides of the bridge: the SQLite row + the
//! on-disk task note's `dispatchedAgentTasks` frontmatter and
//! `completedDate` (or `complete_instances` for recurring).

use std::path::Path;

use agent_proto::service::tasks::AgentTaskQueue;
use agent_proto::tasks::{Queue, status};
use chrono::Utc;
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use tempfile::TempDir;

use agent_dispatch::{DispatchOptions, complete_dispatched, dispatch};
use agent_tasks::{Migrator, Store};
use task::{TaskInfo, parse_str};

async fn setup() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    let store = Store::new(conn);
    store
        .upsert_queue_row(Queue {
            id: "default".into(),
            label: "Default".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .await
        .unwrap();
    (tmp, store)
}

fn write_task_note(vault_root: &Path, rel: &str, body: &str) {
    let abs = vault_root.join(rel);
    if let Some(p) = abs.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(&abs, body).unwrap();
}

#[tokio::test]
async fn dispatch_then_complete_one_shot() {
    let (tmp, store) = setup().await;
    let vault_root = tmp.path().to_path_buf();
    write_task_note(
        &vault_root,
        "tasks/port-kanban.md",
        "---\ntitle: Port kanban view\nstatus: open\npriority: normal\ntags: [task]\n---\n\nPort the kanban view to dx07.\n",
    );
    let raw = std::fs::read_to_string(vault_root.join("tasks/port-kanban.md")).unwrap();
    let mut task = parse_str("tasks/port-kanban.md", "port-kanban", &raw).unwrap();

    let outcome = dispatch(
        &store,
        &vault_root,
        &mut task,
        DispatchOptions {
            queue_id: "default".into(),
            initial_status: status::READY.into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Task note now references the agent task.
    let post = std::fs::read_to_string(&outcome.task_path).unwrap();
    assert!(
        post.contains("dispatchedAgentTasks:"),
        "frontmatter missing dispatchedAgentTasks:\n{post}",
    );
    assert!(post.contains(&outcome.agent_task.id.to_string()));

    // Agent worker claims + completes.
    store
        .claim_agent_task(outcome.agent_task.id.to_string(), "worker-1".into())
        .await
        .unwrap();
    let done = complete_dispatched(
        &store,
        &vault_root,
        &outcome.agent_task.id.to_string(),
        r#"{"shipped": true}"#,
    )
    .await
    .unwrap();
    assert_eq!(done.agent_task.status, status::DONE);

    // Source task note flipped to `done` + has completedDate set.
    let final_raw = std::fs::read_to_string(&done.task_path).unwrap();
    let final_task: TaskInfo =
        parse_str("tasks/port-kanban.md", "port-kanban", &final_raw).unwrap();
    assert_eq!(final_task.status, "done");
    assert!(final_task.completed_date.is_some());
}

#[tokio::test]
async fn dispatch_then_complete_recurring_appends_instance() {
    let (tmp, store) = setup().await;
    let vault_root = tmp.path().to_path_buf();
    write_task_note(
        &vault_root,
        "tasks/daily-digest.md",
        "---\ntitle: Daily digest\nstatus: open\npriority: normal\nrecurrence: \"DTSTART:20260501T070000Z;FREQ=DAILY\"\ntags: [task]\n---\nRun the daily digest.\n",
    );
    let raw = std::fs::read_to_string(vault_root.join("tasks/daily-digest.md")).unwrap();
    let mut task = parse_str("tasks/daily-digest.md", "daily-digest", &raw).unwrap();

    let outcome = dispatch(
        &store,
        &vault_root,
        &mut task,
        DispatchOptions {
            queue_id: "default".into(),
            initial_status: status::READY.into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    store
        .claim_agent_task(outcome.agent_task.id.to_string(), "worker-1".into())
        .await
        .unwrap();
    let done = complete_dispatched(
        &store,
        &vault_root,
        &outcome.agent_task.id.to_string(),
        "{}",
    )
    .await
    .unwrap();
    assert_eq!(done.agent_task.status, status::DONE);

    let final_raw = std::fs::read_to_string(&done.task_path).unwrap();
    let final_task: TaskInfo =
        parse_str("tasks/daily-digest.md", "daily-digest", &final_raw).unwrap();
    // Recurring: stays `open`, gains a complete_instances entry.
    assert_eq!(final_task.status, "open");
    assert!(final_task.completed_date.is_none());
    assert_eq!(final_task.complete_instances.len(), 1);
    let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
    assert_eq!(final_task.complete_instances[0], today);
}
