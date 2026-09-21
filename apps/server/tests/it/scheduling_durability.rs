//! Booking state survives a server restart.
//!
//! Regression cover for the `store-proto` era: the server mounted an
//! in-memory `KvStore`/`LogStore` pair for every hosted org, so the
//! booking audit trail written on `create_booking` /
//! `update_booking_status` was dropped on every process restart. The
//! trail now lives in the vault (`Records/audit/booking-events.jsonl`).
//!
//! This drives the *server* path — a real `Bookings` RPC over the
//! in-process vox link — then throws the whole `AppState` away and
//! boots a fresh one over the same data root, exactly as a restart
//! would, and reads the state back.

// This test exercises services owned by the `scheduling` plugin;
// a build without it has nothing to cover.
#![cfg(feature = "plugin-scheduling")]

use architect::Scope;
use scheduling_proto::{BookingStatus, BookingsClient, EventTypeId, NewBooking};
use task_server::AppState;

#[tokio::test(flavor = "multi_thread")]
async fn bookings_and_audit_trail_survive_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        for var in ["TASK_SERVER_ORG", "TASK_FORGE_POLL_SECS"] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();

    // ── Boot #1: book something through the mounted service. ─────
    let booking_id = {
        let state = AppState::new(None).await.expect("boot AppState");
        let scope = Scope::new();
        let local = state
            .local_server("alpha", &scope)
            .expect("alpha is hosted");
        let bookings: BookingsClient = local.establish().await.expect("establish BookingsClient");

        let booked = bookings
            .create_booking(NewBooking {
                event_type_id: EventTypeId("c30".into()),
                start_utc: "2026-06-01T09:00:00+00:00".into(),
                end_utc: "2026-06-01T09:30:00+00:00".into(),
                attendee_name: "Alice".into(),
                attendee_email: "alice@example.com".into(),
                note: None,
            })
            .await
            .expect("create_booking over the local transport");

        bookings
            .update_booking_status(booked.id.clone(), BookingStatus::Cancelled)
            .await
            .expect("update_booking_status");

        scope.close().await;
        state.scope.close().await;
        booked.id
    };

    // ── Restart: a brand-new AppState over the same data root. ───
    let state = AppState::new(None).await.expect("re-boot AppState");
    let scope = Scope::new();
    let local = state
        .local_server("alpha", &scope)
        .expect("alpha is still hosted");
    let bookings: BookingsClient = local.establish().await.expect("establish BookingsClient");

    let listed = bookings.list_bookings().await.expect("list_bookings");
    assert_eq!(listed.len(), 1, "booking lost across restart");
    assert_eq!(listed[0].id.0, booking_id.0);

    // The audit trail — the state that used to live in `MemStore`.
    let org = state.org("alpha").expect("org state");
    let trail = org.scheduling.booking_audit().expect("read audit trail");
    assert_eq!(trail.len(), 2, "audit trail lost across restart: {trail:?}");
    assert_eq!(trail[0].event, "created");
    assert_eq!(trail[0].booking_id, booking_id.0);
    assert_eq!(trail[1].event, "status");
    assert_eq!(trail[1].status.as_deref(), Some("Cancelled"));

    scope.close().await;
    state.scope.close().await;
}
