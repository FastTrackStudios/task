//! `offline.outbox` end to end: timer calls made with the server
//! unreachable wait in the device's durable queue (a real Turso file),
//! survive the app closing, and land — in order, the refused one named
//! and dropped — when the server comes back.
//!
//! This drives architect's `CallQueue` over `TursoRows` exactly as the app
//! does (`crates/ui/src/offline.rs`), with a transport that makes the same
//! `TimerService` calls over a real WebSocket to this server, through the
//! same error classifier (`architect::classify`) the live write path uses
//! to decide between "queue it" and "roll it back".

use std::sync::Arc;

use architect::{
    CallQueue, CallTransport, Failure, LocalRows, QueuedCall, Settled, TursoRows, classify,
};
use timer_proto::{StartTimerRequest, TimerServiceClient};
use uuid::Uuid;

use crate::support;

/// The app's `TimerCalls`, against a URL: dial, call, classify.
struct OverTheWire {
    url: String,
}

#[async_trait::async_trait]
impl CallTransport for OverTheWire {
    async fn send(&self, call: &QueuedCall) -> Result<Option<String>, Failure> {
        // A dial that fails is the server being unreachable, whatever the
        // transport's own words for it.
        let timer: TimerServiceClient = vox::connect_lane(&self.url)
            .establish()
            .await
            .map_err(|e| Failure::Unreachable(format!("establish `{}`: {e:?}", self.url)))?;
        match call.call.as_str() {
            "timer.start" => {
                let req: StartTimerRequest = serde_json::from_value(call.payload.clone())
                    .map_err(|e| Failure::Refused(e.to_string()))?;
                let s = timer
                    .start_timer(req)
                    .await
                    .map_err(|e| classify(&format!("start timer: {e:?}")))?;
                Ok(Some(s.id.to_string()))
            }
            "timer.stop" => {
                let user: Uuid = serde_json::from_value(call.payload["user_id"].clone())
                    .map_err(|e| Failure::Refused(e.to_string()))?;
                timer
                    .stop_timer(user)
                    .await
                    .map_err(|e| classify(&format!("stop timer: {e:?}")))?;
                Ok(None)
            }
            other => Err(Failure::Refused(format!("unknown call {other}"))),
        }
    }
}

fn start(user: Uuid, what: &str) -> QueuedCall {
    let req = StartTimerRequest {
        user_id: user,
        org_id: Uuid::nil(),
        project_id: None,
        project_path: String::new(),
        task_note_path: String::new(),
        description: what.to_owned(),
    };
    QueuedCall::new(
        "timer.start",
        support::ORG,
        serde_json::to_value(req).expect("encode"),
        what,
    )
    .creating(format!("tmp-{what}"))
}

fn stop(user: Uuid, what: &str) -> QueuedCall {
    QueuedCall::new(
        "timer.stop",
        support::ORG,
        serde_json::json!({ "user_id": user }),
        what,
    )
}

// t[verify offline.outbox]
// t[verify offline.replay]
#[tokio::test(flavor = "multi_thread")]
async fn timer_calls_made_offline_land_in_order_when_the_server_returns() {
    let (state, _data) = support::boot_app_state().await.expect("boot");
    // A port that is closed now and that this server will open later.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr").port()
    };
    let wire = OverTheWire {
        url: format!("ws://127.0.0.1:{port}/org/{}/vox", support::ORG),
    };
    let device = tempfile::tempdir().expect("device dir");
    let db = device.path().join("device.db");
    let db = db.to_str().expect("utf8").to_owned();
    let user = Uuid::new_v4();

    // ── offline: the person starts, stops, stops again, starts again ──
    {
        let rows: Arc<dyn LocalRows> = Arc::new(TursoRows::open_path(&db).await.expect("open"));
        let queue = CallQueue::new(rows);
        for call in [
            start(user, "Mix the bass"),
            stop(user, "Stop the timer"),
            stop(user, "Stop it again"),
            start(user, "Book the room"),
        ] {
            queue.push(call).await.expect("queue");
        }
        let report = queue.replay(&wire).await.expect("replay");
        assert!(
            report.stopped.is_some(),
            "an unreachable server stops the replay"
        );
        assert!(report.settled.is_empty(), "nothing was answered");
        assert_eq!(queue.list().await.expect("list").len(), 4);
    }

    // ── the app closed; the server comes back on the same address ────
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("rebind");
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let rows: Arc<dyn LocalRows> = Arc::new(TursoRows::open_path(&db).await.expect("reopen"));
    let queue = CallQueue::new(rows);
    assert_eq!(
        queue.list().await.expect("list").len(),
        4,
        "the queue survived the app closing"
    );
    let report = queue.replay(&wire).await.expect("replay");
    assert!(report.stopped.is_none(), "{report:?}");
    assert!(
        queue.list().await.expect("list").is_empty(),
        "the queue drained"
    );

    let whats: Vec<(&str, bool)> = report
        .settled
        .iter()
        .map(|s| {
            (
                s.call().what.as_str(),
                matches!(s, Settled::Accepted { .. }),
            )
        })
        .collect();
    assert_eq!(
        whats,
        [
            ("Mix the bass", true),
            ("Stop the timer", true),
            ("Stop it again", false),
            ("Book the room", true),
        ],
        "oldest first; the stop with nothing running was refused and dropped, not retried"
    );
    let first = match report.settled.first() {
        Some(Settled::Accepted { key: Some(key), .. }) => key.clone(),
        other => panic!("the first start reports the server's session id: {other:?}"),
    };

    // The server now holds what the person did offline.
    let timer: TimerServiceClient = vox::connect_lane(&wire.url)
        .establish()
        .await
        .expect("establish");
    let running = timer
        .active_timer(user)
        .await
        .expect("active")
        .expect("the second start is running");
    assert_eq!(running.description, "Book the room");
    assert_ne!(running.id.to_string(), first, "two sessions, not one");
}
