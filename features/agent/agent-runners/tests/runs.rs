//! Run records: every attempt kept, nesting, and the lifecycle.

use agent_proto::run::{FinishRun, RunFilter, RunStatus, StartRun};
use agent_proto::service::runs::Runs;
use agent_runners::{Migrator, RUN_STALE_AFTER, RunStore};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

async fn store() -> (RunStore, DatabaseConnection) {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    (RunStore::new(conn.clone()), conn)
}

fn start(ticket: Uuid, runner: &str) -> StartRun {
    StartRun {
        ticket,
        runner: runner.into(),
        parent: None,
        branch: "agent/abc".into(),
        worktree_path: "/tmp/wt/abc".into(),
        session_path: "/tmp/sessions/abc.json".into(),
    }
}

#[tokio::test]
async fn a_run_records_what_the_runner_reported() {
    let (s, _c) = store().await;
    let ticket = Uuid::new_v4();
    let run = s.start_run(start(ticket, "THEBATTLESHIP")).await.unwrap();

    assert_eq!(run.ticket, ticket);
    assert_eq!(run.runner, "THEBATTLESHIP");
    assert_eq!(run.status, RunStatus::InProgress);
    assert_eq!(run.worktree_path, "/tmp/wt/abc");
    assert_eq!(run.session_path, "/tmp/sessions/abc.json");
    assert!(run.finished_at.is_none());
}

#[tokio::test]
async fn three_failed_attempts_on_one_ticket_stay_three_rows() {
    // The reason this table exists: "this has died three times on the
    // same verify command" must be answerable.
    let (s, _c) = store().await;
    let ticket = Uuid::new_v4();
    for _ in 0..3 {
        let run = s.start_run(start(ticket, "THEBATTLESHIP")).await.unwrap();
        s.finish_run(FinishRun {
            run: run.id,
            passed: false,
            exit_code: Some(101),
            worktree_kept: false,
        })
        .await
        .unwrap();
    }

    let history = s.for_ticket(ticket).await.unwrap();
    assert_eq!(history.len(), 3);
    assert!(history.iter().all(|r| r.status == RunStatus::Failed));
    assert!(history.iter().all(|r| r.exit_code == Some(101)));
}

#[tokio::test]
async fn a_pass_and_a_fail_are_distinguishable() {
    let (s, _c) = store().await;
    let t = Uuid::new_v4();

    let ok = s.start_run(start(t, "r")).await.unwrap();
    let ok = s
        .finish_run(FinishRun {
            run: ok.id,
            passed: true,
            exit_code: Some(0),
            worktree_kept: false,
        })
        .await
        .unwrap();
    assert_eq!(ok.status, RunStatus::Passed);
    assert!(ok.finished_at.is_some());

    let bad = s.start_run(start(t, "r")).await.unwrap();
    let bad = s
        .finish_run(FinishRun {
            run: bad.id,
            passed: false,
            exit_code: Some(1),
            worktree_kept: false,
        })
        .await
        .unwrap();
    assert_eq!(bad.status, RunStatus::Failed);
}

#[tokio::test]
async fn a_kept_worktree_finishes_as_needs_cleanup() {
    let (s, _c) = store().await;
    let run = s.start_run(start(Uuid::new_v4(), "r")).await.unwrap();
    let done = s
        .finish_run(FinishRun {
            run: run.id,
            passed: false,
            exit_code: Some(1),
            worktree_kept: true,
        })
        .await
        .unwrap();

    assert_eq!(done.status, RunStatus::NeedsCleanup);
    assert_eq!(
        done.worktree_path, "/tmp/wt/abc",
        "cleanup needs to know where to look"
    );
}

#[tokio::test]
async fn archiving_reclaims_the_worktree_and_keeps_the_session() {
    let (s, _c) = store().await;
    let run = s.start_run(start(Uuid::new_v4(), "r")).await.unwrap();
    s.finish_run(FinishRun {
        run: run.id,
        passed: true,
        exit_code: Some(0),
        worktree_kept: true,
    })
    .await
    .unwrap();

    let archived = s.archive_run(run.id).await.unwrap();
    assert_eq!(archived.status, RunStatus::Archived);
    assert_eq!(
        archived.session_path, "/tmp/sessions/abc.json",
        "a resumable attempt must stay resumable after cleanup"
    );
}

#[tokio::test]
async fn runs_nest_so_a_manager_can_be_found_from_its_children() {
    let (s, _c) = store().await;
    let manager = s.start_run(start(Uuid::new_v4(), "r")).await.unwrap();

    for _ in 0..2 {
        let mut child = start(Uuid::new_v4(), "r");
        child.parent = Some(manager.id);
        s.start_run(child).await.unwrap();
    }

    let children = s
        .list_runs(RunFilter {
            parent: Some(manager.id),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(children.len(), 2);
    assert!(children.iter().all(|c| c.parent == Some(manager.id)));
}

#[tokio::test]
async fn runs_filter_by_ticket_runner_and_status() {
    let (s, _c) = store().await;
    let t1 = Uuid::new_v4();
    s.start_run(start(t1, "alpha")).await.unwrap();
    s.start_run(start(Uuid::new_v4(), "beta")).await.unwrap();

    let by_ticket = s
        .list_runs(RunFilter {
            ticket: Some(t1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_ticket.len(), 1);

    let by_runner = s
        .list_runs(RunFilter {
            runner: "beta".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_runner.len(), 1);

    let in_progress = s
        .list_runs(RunFilter {
            status: Some(RunStatus::InProgress),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(in_progress.len(), 2);
}

#[tokio::test]
async fn a_lapsed_heartbeat_sweeps_to_stale_and_a_beat_brings_it_back() {
    let (s, conn) = store().await;
    let run = s.start_run(start(Uuid::new_v4(), "r")).await.unwrap();

    // Nothing to sweep while the heartbeat is fresh.
    assert_eq!(s.sweep_stale_runs().await.unwrap(), 0);

    // Age the heartbeat past the window.
    let old = (chrono::Utc::now()
        - chrono::Duration::from_std(RUN_STALE_AFTER).unwrap()
        - chrono::Duration::seconds(5))
    .to_rfc3339();
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE agent_runs SET heartbeat_at = ? WHERE id = ?",
        [old.into(), run.id.to_string().into()],
    ))
    .await
    .unwrap();

    assert_eq!(s.sweep_stale_runs().await.unwrap(), 1);
    assert_eq!(s.get_run(run.id).await.unwrap().status, RunStatus::Stale);

    // Stale is not terminal — a slow runner that comes back resumes.
    s.beat_run(run.id).await.unwrap();
    assert_eq!(
        s.get_run(run.id).await.unwrap().status,
        RunStatus::InProgress
    );
}

#[tokio::test]
async fn a_finished_run_is_never_swept_to_stale() {
    let (s, conn) = store().await;
    let run = s.start_run(start(Uuid::new_v4(), "r")).await.unwrap();
    s.finish_run(FinishRun {
        run: run.id,
        passed: true,
        exit_code: Some(0),
        worktree_kept: false,
    })
    .await
    .unwrap();

    let old = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE agent_runs SET heartbeat_at = ? WHERE id = ?",
        [old.into(), run.id.to_string().into()],
    ))
    .await
    .unwrap();

    assert_eq!(s.sweep_stale_runs().await.unwrap(), 0);
    assert_eq!(s.get_run(run.id).await.unwrap().status, RunStatus::Passed);
}

#[tokio::test]
async fn an_unknown_run_is_reported_not_invented() {
    let (s, _c) = store().await;
    assert!(s.get_run(Uuid::new_v4()).await.is_err());
}
