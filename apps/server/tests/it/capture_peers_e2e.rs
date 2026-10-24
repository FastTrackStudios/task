#![allow(clippy::large_futures)]
//! Capture and process with no host (`offline.peers`, ADR 0006's
//! host-absent stage): two devices' own nodes, the always-on host
//! nowhere, each sweep the one its timer runs (`device_sync::sweep`).
//!
//! A capture is a vault file (`Records/inbox/<id>.md`) and every outcome
//! is a pair of ordinary lane calls (`inbox_proto::Decision`), so what
//! one device decides reaches the other the way any document does:
//! processed into a task on the phone, the laptop sees the task and the
//! capture that names it, words intact; snoozed on the phone, it is put
//! off on the laptop; deleted on the phone, it is gone there too; and a
//! decision the laptop makes on a capture the phone took comes back.

use std::time::Duration;

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use inbox_proto::decision::{self, Decision, Write};
use inbox_proto::{InboxClient, InboxItem, ReviewResponse};
use task_server::local_node::{JoinSpec, LocalNode, NodeOptions};
use vox_core::FromVoxLane;

const ORG: &str = "the-band";

fn opts(root: &std::path::Path) -> NodeOptions {
    NodeOptions {
        owner_name: "Sam Device".into(),
        peers: true,
        ..NodeOptions::at(root)
    }
}

fn client<C: FromVoxLane>(caller: &vox_core::Caller) -> C {
    C::from_vox_lane(caller.clone(), None)
}

async fn sign_in(node: &LocalNode) -> String {
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.expect("anonymous lane");
    let auth: AuthServiceClient = client(&anon);
    auth.sign_in_email_password(SignInEmailPassword {
        email: owner.email.clone(),
        password: owner.password.clone(),
        ip_address: None,
        user_agent: Some("capture-peers-e2e".into()),
    })
    .await
    .expect("sign in on the node")
    .token
}

async fn sweep(node: &LocalNode) {
    let endpoint = node.endpoint(ORG).expect("the band is bound here");
    tokio::time::timeout(
        Duration::from_secs(90),
        task_server::device_sync::sweep(node.state(), ORG, &endpoint),
    )
    .await
    .expect("the sweep did not finish");
}

/// Both ways, then a beat for each node's inbound merge to land.
async fn exchange(a: &LocalNode, b: &LocalNode) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    sweep(b).await;
    sweep(a).await;
}

async fn carry(inbox: &InboxClient, write: Write) {
    match write {
        Write::Save(item) => inbox.upsert_inbox_item(item).await.expect("save"),
        Write::Remove(id) => inbox.delete_inbox_item(id).await.expect("delete"),
    }
}

async fn capture(inbox: &InboxClient, body: &str) -> InboxItem {
    let item = InboxItem::capture(
        uuid::Uuid::new_v4().to_string(),
        body,
        "ui",
        chrono::Utc::now().to_rfc3339(),
    );
    inbox
        .upsert_inbox_item(item.clone())
        .await
        .expect("capture");
    inbox.get_inbox_item(item.id.clone()).await.expect("stored")
}

/// Sweep both ways until `inbox` shows `id` satisfying `cond` (`None`
/// for absent) — what the nodes' timers do, a round at a time, so an
/// edit the backend has not yet folded into the node's document is
/// carried by the next round, as it would be in production.
async fn converge(
    what: &str,
    (a, b): (&LocalNode, &LocalNode),
    inbox: &InboxClient,
    id: &str,
    mut cond: impl FnMut(Option<&InboxItem>) -> bool,
) {
    for _ in 0..20 {
        exchange(a, b).await;
        for _ in 0..20 {
            let items = inbox.list_inbox().await.expect("list the inbox");
            if cond(items.iter().find(|i| i.id == id)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    panic!("timed out waiting for: {what}");
}

// t[verify offline.peers]
// t[verify process.capture.verbatim]
/// Every kind of decision crosses between two nodes with no host.
#[tokio::test(flavor = "multi_thread")]
async fn decisions_on_captures_reach_the_other_device_with_no_host() {
    let env = crate::support::env_lock().await;
    let peer_dir = tempfile::tempdir().unwrap();
    // SAFETY: under the binary's env lock, held while both nodes bind.
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peer_dir.path());
        std::env::set_var("TASK_DEVICE_SYNC_SECS", "0");
        std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
    }
    let (a_root, b_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = LocalNode::open(opts(a_root.path())).await.expect("phone");
    let b = LocalNode::open(opts(b_root.path())).await.expect("laptop");

    let a_token = sign_in(&a).await;
    let mgmt: org_proto::OrgManagementServiceClient = client(&a.server_caller().await.unwrap());
    let band = mgmt
        .create_org(org_proto::CreateOrgRequest {
            session_token: a_token.clone(),
            slug: ORG.into(),
            display_name: "The Band".into(),
            is_home: false,
        })
        .await
        .expect("the phone creates the band's org");
    let a_peer = a.bind(ORG).await.expect("the phone binds the band");
    let joined = b
        .join(JoinSpec {
            slug: ORG.into(),
            id: band.id,
            display_name: band.display_name.clone(),
            peer: Some(a_peer.clone()),
            role: Some("member".into()),
        })
        .await
        .expect("the laptop joins");
    let b_peer = joined
        .endpoint_id
        .expect("the laptop is bound for the band");
    let a_lane = a.caller(ORG, Some(a_token)).await.unwrap();
    let a_sync: files_proto::service::sync::SyncServiceClient = client(&a_lane);
    a_sync
        .enroll_device(b_peer, "laptop".into())
        .await
        .expect("the phone admits the laptop");
    drop(env);

    let b_token = sign_in(&b).await;
    let b_lane = b.caller(ORG, Some(b_token)).await.unwrap();
    let (a_inbox, b_inbox): (InboxClient, InboxClient) = (client(&a_lane), client(&b_lane));
    let (a_tasks, b_tasks): (task::TaskServiceClient, task::TaskServiceClient) =
        (client(&a_lane), client(&b_lane));
    let today = chrono::Utc::now().date_naive();

    // ── on the phone: three captures, three decisions ──
    let body = "Restring the bass before the gig @errands";
    let to_task = capture(&a_inbox, body).await;
    let task = a_tasks
        .create(task::capture(body))
        .await
        .expect("make the task");
    carry(
        &a_inbox,
        Decision::Into(task.path.clone()).apply(&to_task, today),
    )
    .await;
    let to_snooze = capture(&a_inbox, "Learn the harmony on the second chorus").await;
    carry(
        &a_inbox,
        Decision::Later(ReviewResponse::Easy).apply(&to_snooze, today),
    )
    .await;
    let to_delete = capture(&a_inbox, "zzz pocket capture").await;
    carry(&a_inbox, Decision::Delete.apply(&to_delete, today)).await;
    // And one left for the laptop to decide.
    let for_b = capture(&a_inbox, "Send the setlist to the sound engineer").await;

    let pair = (&a, &b);

    // ── on the laptop: each decision arrived ──
    converge(
        "the processed capture on the laptop",
        pair,
        &b_inbox,
        &to_task.id,
        |i| i.is_some_and(|i| i.status == "processed"),
    )
    .await;
    let items = b_inbox.list_inbox().await.expect("list");
    let origin = decision::origin_of(&items, &task.path).expect("the capture names the task");
    assert_eq!(origin.body, body, "its words crossed intact");
    let on_b = b.state().data_root.org(ORG).vault_dir().join(&task.path);
    for _ in 0..300 {
        if on_b.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        b_tasks
            .list()
            .await
            .expect("laptop lists tasks")
            .iter()
            .any(|t| t.path == task.path),
        "the task the capture became is on the laptop"
    );
    converge(
        "the snooze on the laptop",
        pair,
        &b_inbox,
        &to_snooze.id,
        |i| {
            i.is_some_and(|i| {
                i.is_open() && !i.in_review_queue(&today.to_string()) && i.body == to_snooze.body
            })
        },
    )
    .await;
    converge(
        "the delete on the laptop",
        pair,
        &b_inbox,
        &to_delete.id,
        |i| i.is_none(),
    )
    .await;

    // ── the laptop decides the phone's capture; it comes back ──
    converge(
        "the open capture on the laptop",
        pair,
        &b_inbox,
        &for_b.id,
        |i| i.is_some_and(InboxItem::is_open),
    )
    .await;
    let seen = b_inbox
        .get_inbox_item(for_b.id.clone())
        .await
        .expect("laptop reads it");
    carry(&b_inbox, Decision::Done.apply(&seen, today)).await;
    converge(
        "the laptop's decision on the phone",
        pair,
        &a_inbox,
        &for_b.id,
        |i| i.is_some_and(|i| i.status == "processed" && i.processed_into.is_none()),
    )
    .await;

    // ── one due answer on both devices, with no host ──
    // The due queue (`task_proto::due`) is a function of the board, so
    // two devices holding the same board give the same answer — the
    // same count, the same order — and a maintenance mode reorders both
    // alike. Planted on the phone; read on each.
    let today_s = today.to_string();
    let mut late = task::capture("Pay the rehearsal room");
    late.due = Some((today - chrono::Duration::days(1)).to_string());
    let late = a_tasks.create(late).await.expect("a late task");
    let mut chore = task::capture("Coil the cables @chores");
    chore.due = Some(today_s.clone());
    let chore = a_tasks.create(chore).await.expect("a chore due today");
    for t in [&late, &chore] {
        let on_b = b.state().data_root.org(ORG).vault_dir().join(&t.path);
        for _ in 0..20 {
            if on_b.is_file() {
                break;
            }
            exchange(&a, &b).await;
        }
    }
    let ctx =
        task::RelevanceContext::now(today_s.clone(), Some("16:05".into()), None, Some("phone"));
    let due = |tasks: task::TaskServiceClient, ctx: task::RelevanceContext| async move {
        tasks
            .query(task::TaskListFilter {
                relevance: Some(ctx),
                due_now: true,
                ..Default::default()
            })
            .await
            .expect("the due query")
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>()
    };
    let mut on_b = Vec::new();
    for _ in 0..100 {
        on_b = due(b_tasks.clone(), ctx.clone()).await;
        if on_b.contains(&late.id) && on_b.contains(&chore.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let on_a = due(a_tasks.clone(), ctx.clone()).await;
    assert_eq!(on_a, on_b, "the phone and the laptop give one due answer");
    assert!(
        on_a.iter().position(|id| *id == late.id) < on_a.iter().position(|id| *id == chore.id),
        "outside the mode, oldest due leads"
    );
    let mode = ctx.with_session("Maintenance hour", None);
    let (mode_a, mode_b) = (
        due(a_tasks.clone(), mode.clone()).await,
        due(b_tasks.clone(), mode).await,
    );
    assert_eq!(mode_a, mode_b, "and one answer in the maintenance mode");
    assert!(
        mode_a.iter().position(|id| *id == chore.id) < mode_a.iter().position(|id| *id == late.id),
        "in the mode, its chore leads"
    );
    // The inbox half: the same captures due on both.
    let (due_a, due_b) = (
        a_inbox
            .due_queue(today_s.clone())
            .await
            .expect("phone's due captures"),
        b_inbox
            .due_queue(today_s)
            .await
            .expect("laptop's due captures"),
    );
    assert_eq!(
        due_a.iter().map(|i| &i.id).collect::<Vec<_>>(),
        due_b.iter().map(|i| &i.id).collect::<Vec<_>>(),
        "the same captures are due on both devices"
    );
}
