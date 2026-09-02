//! Slice 4 coverage — daily recurring-task dispatch + weekly
//! digest builder.

use std::path::Path;

use agent_proto::service::tasks::AgentTaskQueue;
use agent_proto::tasks::{Queue, status};
use chrono::{Datelike, Duration, NaiveDate, Utc};
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use tempfile::TempDir;

use agent_dispatch::{schedule_recurring, weekly_digest};
use agent_tasks::{Migrator, Store};

async fn setup() -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    let store = Store::new(conn);
    // Seed the queues the cron will route into.
    for slug in ["default", "architect"] {
        store
            .upsert_queue_row(Queue {
                id: slug.into(),
                label: slug.into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
    }
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
async fn schedule_recurring_dispatches_eligible_tasks_once() {
    let (tmp, store) = setup().await;
    let vault_root = tmp.path().to_path_buf();
    // Agent-runnable + scheduled today.
    write_task_note(
        &vault_root,
        "tasks/daily-digest.md",
        "---\ntitle: Daily digest\nstatus: open\npriority: normal\nagentProfile: hermes-research\nscheduled: 2026-05-22\ntags: [task]\n---\nRun the daily digest.\n",
    );
    // Agent-runnable + scheduled in the future. Should NOT dispatch yet.
    write_task_note(
        &vault_root,
        "tasks/future.md",
        "---\ntitle: Future task\nstatus: open\nagentProfile: codex\nscheduled: 2099-12-31\ntags: [task]\n---\nlater\n",
    );
    // Has scheduled today but no agentProfile. Should NOT dispatch.
    write_task_note(
        &vault_root,
        "tasks/human-only.md",
        "---\ntitle: Human only\nstatus: open\nscheduled: 2026-05-22\ntags: [task]\n---\nme\n",
    );

    let today = NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
    let report = schedule_recurring(&store, &vault_root, today)
        .await
        .unwrap();
    assert_eq!(report.inspected, 3, "should inspect all three");
    assert_eq!(report.dispatched.len(), 1, "only daily-digest dispatches");
    assert_eq!(report.dispatched[0].agent_task.title, "Daily digest");

    // Run a second time — idempotent: the daily-digest still
    // has its non-terminal agent task in the queue, so the
    // cron skips it.
    let report2 = schedule_recurring(&store, &vault_root, today)
        .await
        .unwrap();
    assert_eq!(report2.dispatched.len(), 0, "second tick should be a no-op");

    // Complete the digest → next tick re-dispatches.
    let id = report.dispatched[0].agent_task.id.to_string();
    store
        .claim_agent_task(id.clone(), "worker".into())
        .await
        .unwrap();
    store
        .complete_agent_task(id.clone(), "{}".into())
        .await
        .unwrap();
    let report3 = schedule_recurring(&store, &vault_root, today)
        .await
        .unwrap();
    assert_eq!(
        report3.dispatched.len(),
        1,
        "post-completion the cron should pick it up again",
    );
}

#[tokio::test]
async fn weekly_digest_groups_by_project() {
    let (tmp, store) = setup().await;
    let vault_root = tmp.path().to_path_buf();

    // `today` must be the REAL today, not a fixed date. weekly_digest filters
    // on `updated_at`, and complete_agent_task stamps that with wall-clock
    // now — so a hardcoded date (this was 2026-05-22) puts the completed row
    // outside the queried week and the digest returns nothing. The test
    // passed the week it was written and has failed every week since.
    let today = Utc::now().date_naive();
    let scheduled = today.format("%Y-%m-%d").to_string();

    // Two agent-runnable tasks under [[Architect]], one done,
    // one still in flight.
    write_task_note(
        &vault_root,
        "tasks/port-view.md",
        &format!(
            "---\ntitle: Port view to dx07\nstatus: open\nagentProfile: codex\nprojects: [\"[[Architect]]\"]\nscheduled: {scheduled}\ntags: [task]\n---\n"
        ),
    );
    write_task_note(
        &vault_root,
        "tasks/refactor-store.md",
        &format!(
            "---\ntitle: Refactor store\nstatus: open\nagentProfile: codex\nprojects: [\"[[Architect]]\"]\nscheduled: {scheduled}\ntags: [task]\n---\n"
        ),
    );

    let report = schedule_recurring(&store, &vault_root, today)
        .await
        .unwrap();
    assert_eq!(report.dispatched.len(), 2);

    // Complete just one of them.
    // By title, not by position: the dispatch order follows the
    // filesystem's, which differs between machines — CI completed
    // "Refactor store" here and the digest assertions below inverted.
    let port_id = report
        .dispatched
        .iter()
        .find(|d| d.agent_task.title == "Port view to dx07")
        .expect("the port task was dispatched")
        .agent_task
        .id
        .to_string();
    store
        .claim_agent_task(port_id.clone(), "w".into())
        .await
        .unwrap();
    store
        .complete_agent_task(port_id, "{}".into())
        .await
        .unwrap();

    // Weekly digest should include only the completed one.
    let digest = weekly_digest(&store, "architect", today).await.unwrap();
    assert_eq!(digest.completed.len(), 1);
    assert!(digest.completed[0].project.contains("Architect"));
    // The week_of date is the Monday of the input week — computed, not
    // hardcoded, for the same reason `today` is.
    let expected_monday =
        today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    assert_eq!(digest.week_of, expected_monday);
    let md = digest.to_markdown();
    assert!(md.contains("Agent activity"), "markdown:\n{md}");
    assert!(md.contains("Port view to dx07"), "markdown:\n{md}");
    assert!(
        !md.contains("Refactor store"),
        "incomplete task should not appear:\n{md}",
    );
}

#[tokio::test]
async fn weekly_digest_empty_renders_placeholder() {
    let (_tmp, store) = setup().await;
    let digest = weekly_digest(&store, "default", Utc::now().date_naive())
        .await
        .unwrap();
    let md = digest.to_markdown();
    assert!(md.contains("no agent tasks completed"));
}

#[tokio::test]
async fn schedule_recurring_skips_tasks_outside_today_window() {
    let (tmp, store) = setup().await;
    let vault_root = tmp.path().to_path_buf();
    write_task_note(
        &vault_root,
        "tasks/yesterday.md",
        "---\ntitle: Yesterday\nstatus: open\nagentProfile: codex\nscheduled: 2026-05-21\ntags: [task]\n---\n",
    );
    write_task_note(
        &vault_root,
        "tasks/tomorrow.md",
        "---\ntitle: Tomorrow\nstatus: open\nagentProfile: codex\nscheduled: 2026-05-23\ntags: [task]\n---\n",
    );
    let today = NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
    let report = schedule_recurring(&store, &vault_root, today)
        .await
        .unwrap();
    // `yesterday` is past-due → eligible (the cron catches up
    // on missed days). `tomorrow` is future → not eligible.
    assert_eq!(report.dispatched.len(), 1);
    assert_eq!(report.dispatched[0].agent_task.title, "Yesterday");
}

#[tokio::test]
async fn digest_since_returns_completed_only() {
    let (tmp, store) = setup().await;
    let vault_root = tmp.path().to_path_buf();
    write_task_note(
        &vault_root,
        "tasks/done.md",
        "---\ntitle: Done one\nstatus: open\nagentProfile: codex\nscheduled: 2026-05-22\ntags: [task]\n---\n",
    );
    write_task_note(
        &vault_root,
        "tasks/in-flight.md",
        "---\ntitle: In flight\nstatus: open\nagentProfile: codex\nscheduled: 2026-05-22\ntags: [task]\n---\n",
    );
    let today = NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
    let r = schedule_recurring(&store, &vault_root, today)
        .await
        .unwrap();
    assert_eq!(r.dispatched.len(), 2);
    let done_id = r.dispatched[0].agent_task.id.to_string();
    store
        .claim_agent_task(done_id.clone(), "w".into())
        .await
        .unwrap();
    store
        .complete_agent_task(done_id, "{}".into())
        .await
        .unwrap();

    let since = Utc::now() - Duration::hours(1);
    let list = agent_dispatch::digest_since(&store, "default", since)
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].status, status::DONE);
}
