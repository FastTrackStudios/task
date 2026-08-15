//! End-to-end coverage for the claim path + Done-blocked-by-deps
//! transition. These are the bits that would silently corrupt the
//! queue if they regressed, so they get a dedicated test file.

use agent_proto::AgentError;
use agent_proto::service::tasks::AgentTaskQueue;
use agent_proto::tasks::{AgentTask, AgentTaskLink, Labels, Queue, QueueFilter, link_kind, status};
use chrono::Utc;
use sea_orm::{Database, EntityTrait, Set};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

use agent_tasks::entity::{
    AgentTaskActive, AgentTaskEntity, AgentTaskLinkActive, AgentTaskLinkEntity,
};
use agent_tasks::{Migrator, Store};

async fn new_store() -> Store {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    Migrator::up(&conn, None).await.expect("run migrations");
    let store = Store::new(conn);
    store
        .upsert_queue_row(Queue {
            id: "q1".into(),
            label: "Default".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .await
        .expect("seed queue");
    store
}

async fn insert_task(store: &Store, title: &str) -> AgentTask {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let active = AgentTaskActive {
        id: Set(id),
        queue_id: Set("q1".into()),
        title: Set(title.into()),
        description: Set(String::new()),
        status: Set(status::READY.into()),
        priority: Set(2),
        labels: Set(Labels::default()),
        source_task: Set(String::new()),
        project: Set(String::new()),
        agent_profile: Set(String::new()),
        claimed_by: Set(String::new()),
        claim_expires_at: Set(None),
        worker_pid: Set(0),
        result_blob: Set(String::new()),
        linked_session_id: Set(String::new()),
        created_at: Set(now),
        updated_at: Set(now),
        archived_at: Set(None),
    };
    AgentTaskEntity::insert(active)
        .exec(store.conn())
        .await
        .expect("insert");
    AgentTaskEntity::find_by_id(id)
        .one(store.conn())
        .await
        .unwrap()
        .map(|m| AgentTask {
            id: m.id,
            queue_id: m.queue_id,
            title: m.title,
            description: m.description,
            status: m.status,
            priority: m.priority,
            labels: m.labels,
            source_task: m.source_task,
            project: m.project,
            agent_profile: m.agent_profile,
            claimed_by: m.claimed_by,
            claim_expires_at: m.claim_expires_at,
            worker_pid: m.worker_pid,
            result_blob: m.result_blob,
            linked_session_id: m.linked_session_id,
            created_at: m.created_at,
            updated_at: m.updated_at,
            archived_at: m.archived_at,
        })
        .unwrap()
}

async fn add_link(store: &Store, from: Uuid, to: Uuid, kind: &str) -> AgentTaskLink {
    let id = Uuid::new_v4();
    let active = AgentTaskLinkActive {
        id: Set(id),
        from_task: Set(from),
        to_task: Set(to),
        kind: Set(kind.into()),
        created_at: Set(Utc::now()),
    };
    AgentTaskLinkEntity::insert(active)
        .exec(store.conn())
        .await
        .expect("insert link");
    AgentTaskLink {
        id,
        from_task: from,
        to_task: to,
        kind: kind.into(),
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn claim_succeeds_then_second_claim_conflicts() {
    let store = new_store().await;
    let t = insert_task(&store, "first").await;
    let claimed = store
        .claim_agent_task(t.id.to_string(), "worker-a".into())
        .await
        .unwrap();
    assert_eq!(claimed.status, status::RUNNING);
    assert_eq!(claimed.claimed_by, "worker-a");
    let err = store
        .claim_agent_task(t.id.to_string(), "worker-b".into())
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::Conflict(_)));
}

#[tokio::test]
async fn cannot_set_running_directly() {
    let store = new_store().await;
    let t = insert_task(&store, "x").await;
    let err = store
        .set_agent_task_status(t.id.to_string(), status::RUNNING.into())
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::Invalid(_)));
}

#[tokio::test]
async fn cannot_complete_while_blocked_by_dep() {
    let store = new_store().await;
    let parent = insert_task(&store, "parent").await;
    let child = insert_task(&store, "child").await;
    add_link(&store, parent.id, child.id, link_kind::BLOCKS).await;
    let err = store
        .complete_agent_task(child.id.to_string(), "{}".into())
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::Invalid(_)));

    store
        .complete_agent_task(parent.id.to_string(), "{}".into())
        .await
        .unwrap();
    let done = store
        .complete_agent_task(child.id.to_string(), "{}".into())
        .await
        .unwrap();
    assert_eq!(done.status, status::DONE);
}

#[tokio::test]
async fn done_clears_claim_state() {
    let store = new_store().await;
    let t = insert_task(&store, "x").await;
    store
        .claim_agent_task(t.id.to_string(), "worker-a".into())
        .await
        .unwrap();
    let done = store
        .complete_agent_task(t.id.to_string(), r#"{"ok":true}"#.into())
        .await
        .unwrap();
    assert_eq!(done.status, status::DONE);
    assert_eq!(done.claimed_by, "");
    assert!(done.claim_expires_at.is_none());
    assert_eq!(done.result_blob, r#"{"ok":true}"#);
}

#[tokio::test]
async fn filter_excludes_archived_by_default() {
    let store = new_store().await;
    let a = insert_task(&store, "a").await;
    let b = insert_task(&store, "b").await;
    store
        .set_agent_task_status(b.id.to_string(), status::ARCHIVED.into())
        .await
        .unwrap();
    let snap = store
        .read_queue("q1".into(), QueueFilter::default())
        .await
        .unwrap();
    assert_eq!(snap.tasks.len(), 1, "archived should be hidden");
    assert_eq!(snap.tasks[0].id, a.id);

    let with_arch = QueueFilter {
        include_archived: true,
        ..Default::default()
    };
    let snap2 = store.read_queue("q1".into(), with_arch).await.unwrap();
    assert_eq!(snap2.tasks.len(), 2);
}
