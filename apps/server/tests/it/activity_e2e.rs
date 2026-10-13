#![allow(clippy::large_futures)]
//! End-to-end cover for the activity feed: writes over the mounted
//! services → the notifier's activity pumps (and the comment hook) →
//! entries on the mounted `ActivityFeed` RPC and its stream.
//!
//! Two tests, both through the org router:
//!
//! - [`the_feed_records_what_happened_end_to_end`] over the in-process
//!   transport: a task's creation and status change (from → to), a
//!   comment, a capture processed into a task, a "Maintenance hour"
//!   timer as a mode — then the entity / project filters and the
//!   `before` cursor, and the live stream.
//! - [`the_feed_names_the_signed_in_writer`] over the real WebSocket
//!   with a session bearer: the permission gate's caller is who the
//!   entry names (`activity.actor`).

use std::time::Duration;

use architect::Scope;
use notify_proto::{
    Activity, ActivityCursor, ActivityEvent, ActivityFeedClient, ActivityFeedStreamClient,
    ActivityFilter, ActivityKind,
};
use task_server::AppState;
use uuid::Uuid;

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

/// Poll `list(filter)` until `done` holds or 10 s pass.
async fn wait_for(
    feed: &ActivityFeedClient,
    filter: ActivityFilter,
    done: impl Fn(&[Activity]) -> bool,
) -> Vec<Activity> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let rows = feed.list(filter.clone()).await.expect("list activity");
        if done(&rows) {
            return rows;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "activity never arrived; have: {:#?}",
            rows.iter()
                .map(|a| (a.kind, a.entity_title.clone()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn has(rows: &[Activity], kind: ActivityKind) -> bool {
    rows.iter().any(|a| a.kind == kind)
}

// t[verify activity.task.diff]
// t[verify activity.comment]
// t[verify activity.inbox]
// t[verify activity.timer.mode]
// t[verify activity.feed.filter]
// t[verify activity.feed.order]
// t[verify activity.feed.live]
#[tokio::test(flavor = "multi_thread")]
async fn the_feed_records_what_happened_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let env_guard = crate::support::env_lock().await;
    // SAFETY: held under the env lock while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        for var in [
            "TASK_SERVER_ORG",
            "TASK_SERVER_VAULT_ROOT",
            "TASK_NOTIFY_WEBHOOK",
            "TASK_SERVER_NOTIFY_URL",
        ] {
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
    let local = state.local_server("alpha", &scope).expect("alpha");

    // Subscribe before writing anything.
    let stream: ActivityFeedStreamClient = local.establish().await.expect("stream client");
    let (tx, mut rx) = vox::channel::<ActivityEvent>();
    let _sub = tokio::spawn(async move {
        let _ = stream.events(tx).await;
    });
    // Let the notifier's own subscriptions attach and seed.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let feed: ActivityFeedClient = local.establish().await.expect("feed client");
    let tasks: task::TaskServiceClient = local.establish().await.expect("task client");

    // ── a task, moved along ──
    let project = Uuid::new_v4();
    let mut draft = task::capture("Comp lead vocals");
    draft.status = "open".into();
    draft.project_id = Some(project);
    let created = tasks.create(draft).await.expect("create");
    let mut moved = created.clone();
    moved.status = "in-progress".into();
    tasks.update(moved).await.expect("update");

    let rows = wait_for(&feed, ActivityFilter::entity("task", created.id), |r| {
        has(r, ActivityKind::Created) && has(r, ActivityKind::StatusChanged)
    })
    .await;
    let status = rows
        .iter()
        .find(|a| a.kind == ActivityKind::StatusChanged)
        .unwrap();
    assert_eq!(
        (status.from.as_str(), status.to.as_str()),
        ("open", "in-progress")
    );
    assert_eq!(status.field, "status");
    assert_eq!(status.href, format!("/tasks/{}", created.id));
    assert_eq!(status.entity_title, "Comp lead vocals");
    assert_eq!(status.project_id, Some(project));

    // The stream delivered it too.
    loop {
        if let ActivityEvent::Recorded(a) = next_event(&mut rx, 10).await
            && a.kind == ActivityKind::StatusChanged
            && a.entity_id == created.id.to_string()
        {
            break;
        }
    }

    // ── a comment on it ──
    let threads: threads::ThreadsServiceClient = local.establish().await.expect("threads");
    let thread = threads
        .create_thread(threads::CreateThreadRequest {
            org_id: Uuid::nil(),
            entity_type: "task".into(),
            entity_id: created.id,
            title: "Comments".into(),
            kind: String::new(),
            created_by: Uuid::nil(),
            source_kind: "native".into(),
            source_ref: None,
            source_url: None,
        })
        .await
        .expect("create thread");
    threads
        .post_message(threads::PostMessageRequest {
            thread_id: thread.id,
            org_id: Uuid::nil(),
            author_id: None,
            author_label: "Sam".into(),
            body: "Take three has the best chorus.".into(),
            reply_to: None,
            source_kind: "native".into(),
            external_id: None,
            original_text: None,
            source_url: None,
            posted_at: None,
        })
        .await
        .expect("post");
    let rows = wait_for(&feed, ActivityFilter::entity("task", created.id), |r| {
        has(r, ActivityKind::Commented)
    })
    .await;
    let comment = rows
        .iter()
        .find(|a| a.kind == ActivityKind::Commented)
        .unwrap();
    assert_eq!(comment.actor_name, "Sam");
    assert_eq!(comment.body, "Take three has the best chorus.");
    assert_eq!(comment.entity_title, "Comp lead vocals");
    assert_eq!(
        comment.project_id,
        Some(project),
        "a comment joins its project's feed"
    );

    // ── capture → process ──
    let inbox: inbox_proto::InboxClient = local.establish().await.expect("inbox");
    let capture = inbox_proto::InboxItem::capture(
        Uuid::new_v4().to_string(),
        "Call the piano tuner about Thursday",
        "ui",
        chrono::Utc::now().to_rfc3339(),
    );
    inbox
        .upsert_inbox_item(capture.clone())
        .await
        .expect("capture");
    let made = tasks
        .create(task::capture("Call the piano tuner"))
        .await
        .expect("task from capture");
    let mut done = capture.clone();
    done.status = inbox_proto::InboxItem::STATUS_PROCESSED.into();
    done.processed_into = Some(made.path.clone());
    inbox.upsert_inbox_item(done).await.expect("process");
    let rows = wait_for(&feed, ActivityFilter::entity("inbox", &capture.id), |r| {
        has(r, ActivityKind::Captured) && has(r, ActivityKind::Processed)
    })
    .await;
    let processed = rows
        .iter()
        .find(|a| a.kind == ActivityKind::Processed)
        .unwrap();
    assert_eq!(processed.to, "Call the piano tuner");
    assert_eq!(processed.href, format!("/tasks/{}", made.id));
    assert_eq!(
        processed.entity_title,
        "Call the piano tuner about Thursday"
    );

    // ── a maintenance hour, started and ended ──
    let timer: timer_proto::TimerServiceClient = local.establish().await.expect("timer");
    let user = Uuid::new_v4();
    let session = timer
        .start_timer(timer_proto::StartTimerRequest {
            user_id: user,
            org_id: Uuid::nil(),
            project_id: None,
            project_path: String::new(),
            task_note_path: String::new(),
            description: "Maintenance hour".into(),
        })
        .await
        .expect("start");
    timer.stop_timer(user).await.expect("stop");
    let rows = wait_for(&feed, ActivityFilter::entity("timer", session.id), |r| {
        has(r, ActivityKind::ModeStarted) && has(r, ActivityKind::ModeEnded)
    })
    .await;
    assert!(rows.iter().all(|a| a.entity_title == "Maintenance hour"));

    // ── the filters ──
    let in_project = feed
        .list(ActivityFilter::project(project))
        .await
        .expect("project feed");
    assert!(
        in_project
            .iter()
            .all(|a| a.project_id == Some(project) && a.entity_id == created.id.to_string()),
        "a project's feed is its tasks' entries and nothing else"
    );
    assert!(has(&in_project, ActivityKind::Commented));
    let only_comments = feed
        .list(ActivityFilter {
            kinds: vec![ActivityKind::Commented],
            ..Default::default()
        })
        .await
        .expect("kinds");
    assert_eq!(only_comments.len(), 1);

    // ── scroll-back: pages of two tile the whole feed ──
    let all = feed.list(ActivityFilter::default()).await.expect("all");
    assert!(all.len() >= 8, "every write above made at least one entry");
    assert!(all.windows(2).all(|w| w[0].at >= w[1].at), "newest first");
    let mut tiled = Vec::new();
    let mut before = None;
    loop {
        let page = feed
            .list(ActivityFilter {
                before,
                limit: Some(2),
                ..Default::default()
            })
            .await
            .expect("page");
        let Some(last) = page.last() else { break };
        before = Some(ActivityCursor::after(last));
        tiled.extend(page.iter().map(|a| a.id));
    }
    assert_eq!(tiled, all.iter().map(|a| a.id).collect::<Vec<_>>());

    scope.close().await;
    state.scope.close().await;
}

// t[verify activity.actor]
#[tokio::test(flavor = "multi_thread")]
async fn the_feed_names_the_signed_in_writer() -> eyre::Result<()> {
    use architect_auth::CreateEmailPasswordUser;
    use task_server::{AuthState, capability::ServerKeypair, router};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = crate::support::env_lock().await;
    // SAFETY: held under the env lock while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::remove_var("TASK_ENFORCE_PERMISSIONS");
        std::env::remove_var("TASK_SERVER_NOTIFY_URL");
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org("activity-test", "Activity Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let bundle = auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "alice@acme.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Alice".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;
    // The notifier after the account exists, so its directory names her.
    task_server::notifier::spawn(&state);
    let scope = Scope::new();
    let local = state.local_server("activity-test", &scope).expect("org");
    let feed: ActivityFeedClient = local.establish().await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut request = format!("ws://127.0.0.1:{port}/vox").into_client_request()?;
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("vox.v1, vox.bearer.{}", bundle.token).parse()?,
    );
    let (stream, _response) = tokio_tungstenite::connect_async(request).await?;
    let tasks: task::TaskServiceClient = vox_core::initiator_on(vox_websocket::WsLink::new(stream))
        .establish::<task::TaskServiceClient>()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))?;

    let mut draft = task::capture("Master review with the label");
    draft.status = "open".into();
    let created = tasks
        .create(draft)
        .await
        .map_err(|e| eyre::eyre!("{e:?}"))?;
    let mut moved = created.clone();
    moved.status = "done".into();
    tasks
        .update(moved)
        .await
        .map_err(|e| eyre::eyre!("{e:?}"))?;

    let rows = wait_for(&feed, ActivityFilter::entity("task", created.id), |r| {
        has(r, ActivityKind::Completed)
    })
    .await;
    let done = rows
        .iter()
        .find(|a| a.kind == ActivityKind::Completed)
        .unwrap();
    assert_eq!(done.actor_name, "Alice", "the gate's caller is the byline");
    assert_eq!(done.actor_id, bundle.user.id.to_string());
    scope.close().await;
    Ok(())
}
