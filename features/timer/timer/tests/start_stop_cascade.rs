//! Active-timer invariant + rate cascade end-to-end.

use std::sync::Arc;

use chrono::{Duration, Utc};
use sea_orm::{Database, EntityTrait, Set};
use sea_orm_migration::MigratorTrait;
use timer_proto::TimerError;
use timer_proto::service::{LogSessionRequest, RateSource, StartTimerRequest, TimerService};
use uuid::Uuid;

use timer::entity::{
    OrgMemberRateActive, OrgMemberRateEntity, ProjectMemberRateActive, ProjectMemberRateEntity,
};
use timer::{Migrator, Store};

fn defaults_with(
    project_id: Uuid,
    rate: i64,
    currency: &str,
    billable: bool,
) -> timer::store::StaticProjectDefaults {
    let mut map = std::collections::HashMap::new();
    map.insert(project_id, (rate, currency.into(), billable));
    timer::store::StaticProjectDefaults { map }
}

async fn new_store(defaults: timer::store::StaticProjectDefaults) -> Store {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&conn, None).await.unwrap();
    Store::new(conn, Arc::new(defaults))
}

fn req(user_id: Uuid, org_id: Uuid, project_id: Option<Uuid>) -> StartTimerRequest {
    StartTimerRequest {
        user_id,
        org_id,
        project_id,
        project_path: project_id
            .map(|p| format!("Projects/{p}.md"))
            .unwrap_or_default(),
        task_note_path: String::new(),
        description: "test".into(),
    }
}

#[tokio::test]
async fn start_then_stop_round_trip() {
    let store = new_store(timer::store::StaticProjectDefaults::default()).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    let started = store.start_timer(req(user, org, None)).await.unwrap();
    assert!(started.end_time.is_none(), "fresh start is open");

    let active = store.active_timer(user).await.unwrap();
    assert_eq!(active.as_ref().map(|s| s.id), Some(started.id));

    let stopped = store.stop_timer(user).await.unwrap();
    assert!(stopped.end_time.is_some());
    assert!(store.active_timer(user).await.unwrap().is_none());
}

#[tokio::test]
async fn second_start_conflicts_with_already_running() {
    let store = new_store(timer::store::StaticProjectDefaults::default()).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    store.start_timer(req(user, org, None)).await.unwrap();
    let err = store.start_timer(req(user, org, None)).await.unwrap_err();
    assert!(matches!(err, TimerError::AlreadyRunning(_)));
}

#[tokio::test]
async fn stop_without_active_timer_errors() {
    let store = new_store(timer::store::StaticProjectDefaults::default()).await;
    let user = Uuid::new_v4();
    let err = store.stop_timer(user).await.unwrap_err();
    assert!(matches!(err, TimerError::NoActiveTimer(_)));
}

#[tokio::test]
async fn switch_closes_then_opens_in_one_call() {
    let store = new_store(timer::store::StaticProjectDefaults::default()).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    let first = store.start_timer(req(user, org, None)).await.unwrap();
    let (closed, opened) = store.switch_timer(req(user, org, None)).await.unwrap();
    assert_eq!(closed.unwrap().id, first.id);
    assert!(opened.end_time.is_none());
    // And the new one is the active session.
    assert_eq!(
        store.active_timer(user).await.unwrap().unwrap().id,
        opened.id
    );
}

#[tokio::test]
async fn switch_with_no_existing_session_just_opens() {
    let store = new_store(timer::store::StaticProjectDefaults::default()).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    let (closed, opened) = store.switch_timer(req(user, org, None)).await.unwrap();
    assert!(closed.is_none());
    assert!(opened.end_time.is_none());
}

#[tokio::test]
async fn cascade_picks_project_member_when_present() {
    let project = Uuid::new_v4();
    let store = new_store(defaults_with(project, 10000, "USD", true)).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();

    // ProjectMemberRate row for this user@project = $150/hr.
    ProjectMemberRateEntity::insert(ProjectMemberRateActive {
        id: Set(Uuid::new_v4()),
        project_id: Set(project),
        user_id: Set(user),
        hourly_cents: Set(15000),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    })
    .exec(store.conn())
    .await
    .unwrap();

    store
        .start_timer(req(user, org, Some(project)))
        .await
        .unwrap();
    let stopped = store.stop_timer(user).await.unwrap();
    assert_eq!(stopped.rate_cents, 15000, "ProjectMember wins");
    assert_eq!(stopped.currency, "USD");
}

#[tokio::test]
async fn cascade_falls_through_to_project_default() {
    let project = Uuid::new_v4();
    let store = new_store(defaults_with(project, 12000, "USD", true)).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    // No ProjectMemberRate row → project default kicks in.
    store
        .start_timer(req(user, org, Some(project)))
        .await
        .unwrap();
    let stopped = store.stop_timer(user).await.unwrap();
    assert_eq!(stopped.rate_cents, 12000);
}

#[tokio::test]
async fn cascade_falls_through_to_org_member_rate() {
    let project = Uuid::new_v4();
    // Project default is 0 → cascade continues.
    let store = new_store(defaults_with(project, 0, "EUR", true)).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();

    OrgMemberRateEntity::insert(OrgMemberRateActive {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(user),
        hourly_cents: Set(9500),
        currency: Set("EUR".into()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    })
    .exec(store.conn())
    .await
    .unwrap();

    store
        .start_timer(req(user, org, Some(project)))
        .await
        .unwrap();
    let stopped = store.stop_timer(user).await.unwrap();
    assert_eq!(stopped.rate_cents, 9500);
    assert_eq!(stopped.currency, "EUR");
}

#[tokio::test]
async fn non_billable_zeroes_rate_even_with_cascade_hits() {
    let project = Uuid::new_v4();
    // Project marked non-billable.
    let store = new_store(defaults_with(project, 12000, "USD", false)).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    store
        .start_timer(req(user, org, Some(project)))
        .await
        .unwrap();
    let stopped = store.stop_timer(user).await.unwrap();
    assert!(!stopped.billable);
    assert_eq!(stopped.rate_cents, 0);
    assert_eq!(stopped.currency, "");
}

#[tokio::test]
async fn log_session_skips_active_invariant() {
    let store = new_store(timer::store::StaticProjectDefaults::default()).await;
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    // Start a current timer.
    store.start_timer(req(user, org, None)).await.unwrap();
    // Then retro-log an entry from yesterday — no conflict.
    let yesterday = Utc::now() - Duration::days(1);
    let logged = store
        .log_session(LogSessionRequest {
            user_id: user,
            org_id: org,
            project_id: None,
            project_path: String::new(),
            task_note_path: String::new(),
            description: "forgot to start the timer".into(),
            start_time: yesterday,
            end_time: yesterday + Duration::hours(2),
            billable_override: Some(false),
        })
        .await
        .unwrap();
    assert!(logged.end_time.is_some());
    // Active timer still the open one.
    assert!(store.active_timer(user).await.unwrap().is_some());
}

#[tokio::test]
async fn resolve_rate_surface() {
    let project = Uuid::new_v4();
    let store = new_store(defaults_with(project, 12000, "USD", true)).await;
    let user = Uuid::new_v4();
    let res = store.resolve_rate(user, Some(project)).await.unwrap();
    assert_eq!(res.hourly_cents, 12000);
    assert_eq!(res.source, RateSource::ProjectDefault);
}
