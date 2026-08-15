//! End-to-end cover for the non-task `#[subscribe]` entity streams:
//! mutate over the mounted RPC service, receive the event on the
//! mounted stream sibling — the whole realtime path through the org
//! router (dispatcher → backend hub → stream host → subscriber
//! channel), over the in-process vox transport.
//!
//! Two slices are exercised against one booted `AppState`:
//! - **project** — the flagship entity-CRUD conversion
//!   (`ProjectEvent::Upserted` on create, `::Deleted` on delete);
//! - **scheduling** — the slice-wide subscribe-only stream
//!   (`SchedulingEvent::BookingUpserted` from a `Bookings` RPC),
//!   proving one hub serves events published by a *different*
//!   capability service than the stream mount.
//!
//! Self-sandboxed: tempdir data root via `TASK_DATA_ROOT`, one test
//! per binary so the env setup races nothing.

// This test exercises services owned by the `scheduling` plugin;
// a build without it has nothing to cover.
#![cfg(feature = "plugin-scheduling")]

use std::time::Duration;

use architect::Scope;
use project::{ProjectEvent, ProjectServiceClient, ProjectServiceStreamClient};
use scheduling_proto::{
    BookingsClient, EventTypeId, NewBooking, SchedulingEvent, SchedulingEventsStreamClient,
};
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
async fn entity_streams_deliver_mutations_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
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
    let scope = Scope::new();
    let local = state
        .local_server("alpha", &scope)
        .expect("alpha is hosted");

    // ── project: subscribe, then create + delete over RPC ────────
    let stream: ProjectServiceStreamClient =
        local.establish().await.expect("project stream client");
    let (tx, mut rx) = vox::channel::<ProjectEvent>();
    let _sub = tokio::spawn(async move {
        let _ = stream.events(tx).await;
    });
    // Let the subscribe attach before the first write (the hub
    // replays nothing — the contract is subscribe-then-fetch).
    tokio::time::sleep(Duration::from_millis(50)).await;

    let projects: ProjectServiceClient = local.establish().await.expect("project client");
    let mut draft = project::ProjectInfo {
        id: uuid::Uuid::nil(),
        path: String::new(),
        title: "Realtime".into(),
        status: "active".into(),
        priority: "normal".into(),
        project_type: "general".into(),
        lead: String::new(),
        tags: project::model::Tags::default(),
        parent_id: None,
        same_as: None,
        target_date: None,
        progress_percent: -1,
        details: String::new(),
        client_id: None,
        billable_default: false,
        currency: String::new(),
        default_rate_cents: 0,
        estimated_seconds: 0,
        agent_profile: String::new(),
        verify_command: String::new(),
        color: String::new(),
        image: String::new(),
        archived: false,
        states: None,
        date_created: None,
        date_modified: None,
    };
    let created = projects
        .create(draft.clone())
        .await
        .expect("create project");
    match next_event(&mut rx, 5).await {
        ProjectEvent::Upserted(p) => {
            assert_eq!(p.id, created.id);
            assert_eq!(p.title, "Realtime");
            assert!(!p.path.is_empty(), "event carries the assigned path");
        }
        other => panic!("expected Upserted, got {other:?}"),
    }

    draft.id = created.id;
    draft.title = "Realtime v2".into();
    let updated = projects.update(draft).await.expect("update project");
    match next_event(&mut rx, 5).await {
        ProjectEvent::Upserted(p) => {
            assert_eq!(p.id, updated.id);
            assert_eq!(p.title, "Realtime v2", "full post-write state");
        }
        other => panic!("expected Upserted, got {other:?}"),
    }

    projects.delete(created.id).await.expect("delete project");
    match next_event(&mut rx, 5).await {
        ProjectEvent::Deleted(id) => assert_eq!(id, created.id),
        other => panic!("expected Deleted, got {other:?}"),
    }

    // ── scheduling: the slice's one stream, fed by Bookings ──────
    let sched_stream: SchedulingEventsStreamClient =
        local.establish().await.expect("scheduling stream client");
    let (stx, mut srx) = vox::channel::<SchedulingEvent>();
    let _ssub = tokio::spawn(async move {
        let _ = sched_stream.events(stx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let bookings: BookingsClient = local.establish().await.expect("bookings client");
    let booked = bookings
        .create_booking(NewBooking {
            event_type_id: EventTypeId("c30".into()),
            start_utc: "2026-08-01T09:00:00+00:00".into(),
            end_utc: "2026-08-01T09:30:00+00:00".into(),
            attendee_name: "Alice".into(),
            attendee_email: "alice@example.com".into(),
            note: None,
        })
        .await
        .expect("create_booking");
    match next_event(&mut srx, 5).await {
        SchedulingEvent::BookingUpserted(b) => {
            assert_eq!(b.id.0, booked.id.0);
            assert_eq!(b.attendee_name, "Alice");
        }
        other => panic!("expected BookingUpserted, got {other:?}"),
    }

    scope.close().await;
    state.scope.close().await;
}
