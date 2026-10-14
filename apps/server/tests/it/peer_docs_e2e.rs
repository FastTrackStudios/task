#![allow(clippy::large_futures)]
//! `offline.peers` end to end: documents move between two nodes with no
//! host anywhere — two devices' own nodes, dialling each other by
//! endpoint id over iroh (addresses through `TASK_IROH_PEER_DIR`, the
//! offline stand-in for discovery), each sweep the one the timer runs in
//! production (`device_sync::sweep`).
//!
//! A note written on one reaches the other as a file; a task made on the
//! other reaches the first and shows in its task list; a note edited on
//! both while apart converges to one text holding both edits, on both.

use std::time::Duration;

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use task_server::local_node::{JoinSpec, LocalNode, NodeOptions};
use vault_proto::{IfMatch, VaultSyncClient};
use vox_core::FromVoxLane;

const ORG: &str = "the-band";
const NOTE: &str = "Setlist.md";

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
        user_agent: Some("peer-docs-e2e".into()),
    })
    .await
    .expect("sign in on the node")
    .token
}

/// One pass of `node`'s sweep of the band's peers — what its timer runs.
async fn sweep(node: &LocalNode) {
    let endpoint = node.endpoint(ORG).expect("the band is bound here");
    tokio::time::timeout(
        Duration::from_secs(90),
        task_server::device_sync::sweep(node.state(), ORG, &endpoint),
    )
    .await
    .expect("the sweep did not finish");
}

fn note_on(node: &LocalNode) -> Option<String> {
    let vault = node.state().data_root.org(ORG).vault_dir();
    std::fs::read_to_string(vault.join(NOTE)).ok()
}

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..300 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

// t[verify offline.peers] — two nodes, no host: a note and a task move
// between them, and a note edited on both converges on both
#[tokio::test(flavor = "multi_thread")]
async fn documents_move_between_two_nodes_with_no_host() {
    let env = crate::support::env_lock().await;
    let peer_dir = tempfile::tempdir().unwrap();
    // SAFETY: under the binary's env lock, held while both nodes bind.
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peer_dir.path());
        // Sweeps run when this test says, not on a timer.
        std::env::set_var("TASK_DEVICE_SYNC_SECS", "0");
        // Enforced, not observed: each person has to be a member of the
        // band's org on their own node (A made it, B joined it) and each
        // node a peer the other admits, or the calls below are refused.
        std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
    }
    let (a_root, b_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = LocalNode::open(opts(a_root.path()))
        .await
        .expect("device A");
    let b = LocalNode::open(opts(b_root.path()))
        .await
        .expect("device B");

    // The band's org, made on A; B takes a node of it; each admits the
    // other — the join handshake, exactly as `local_node_e2e` runs it.
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
        .expect("A creates the band's org");
    let a_peer = a.bind(ORG).await.expect("A binds the band");
    let joined = b
        .join(JoinSpec {
            slug: ORG.into(),
            id: band.id,
            display_name: band.display_name.clone(),
            peer: Some(a_peer.clone()),
            role: Some("member".into()),
        })
        .await
        .expect("B joins");
    let b_peer = joined.endpoint_id.expect("B is bound for the band");
    let a_lane = a.caller(ORG, Some(a_token)).await.unwrap();
    let a_sync: files_proto::service::sync::SyncServiceClient = client(&a_lane);
    a_sync
        .enroll_device(b_peer, "device B".into())
        .await
        .expect("A admits B");
    drop(env);

    let b_token = sign_in(&b).await;
    let b_lane = b.caller(ORG, Some(b_token)).await.unwrap();
    let a_vault: VaultSyncClient = client(&a_lane);
    let b_vault: VaultSyncClient = client(&b_lane);
    let b_tasks: task::TaskServiceClient = client(&b_lane);
    let a_tasks: task::TaskServiceClient = client(&a_lane);

    // ── A writes a note; B makes a task ──
    let setlist = "---\ntitle: Setlist\n---\nOpener\n";
    a_vault
        .put_file(
            "default".into(),
            NOTE.into(),
            setlist.into(),
            IfMatch::CreateOnly,
        )
        .await
        .expect("A writes the setlist");
    let made = b_tasks
        .create(task::capture("Rehearse the bridge"))
        .await
        .expect("B makes a task");

    // ── each node's sweep: B pulls A, A pulls B ──
    sweep(&b).await;
    sweep(&a).await;
    eventually("the setlist on B", || {
        note_on(&b).as_deref() == Some(setlist)
    })
    .await;
    let on_a = a.state().data_root.org(ORG).vault_dir().join(&made.path);
    eventually("B's task as a file on A", || on_a.is_file()).await;
    let listed = a_tasks.list().await.expect("A lists tasks");
    assert!(
        listed
            .iter()
            .any(|t| t.id == made.id && t.title == made.title),
        "B's task is one of A's: {:?}",
        listed.iter().map(|t| &t.title).collect::<Vec<_>>()
    );

    // ── both edit the setlist apart ──
    let on_a_now = note_on(&a).expect("A's setlist");
    let on_b_now = note_on(&b).expect("B's setlist");
    a_vault
        .put_file(
            "default".into(),
            NOTE.into(),
            on_a_now.replace("Opener\n", "Opener\nClosing\n").into(),
            IfMatch::Force,
        )
        .await
        .expect("A adds the closer");
    b_vault
        .put_file(
            "default".into(),
            NOTE.into(),
            on_b_now.replace("Opener\n", "Soundcheck\nOpener\n").into(),
            IfMatch::Force,
        )
        .await
        .expect("B adds the soundcheck");

    // Each node's document takes its own edit before the sweeps (the
    // inbound merge) — then the sweeps exchange them.
    tokio::time::sleep(Duration::from_millis(300)).await;
    sweep(&b).await;
    sweep(&a).await;
    let want = "---\ntitle: Setlist\n---\nSoundcheck\nOpener\nClosing\n";
    eventually("A's setlist holds both edits", || {
        note_on(&a).as_deref() == Some(want)
    })
    .await;
    eventually("B's setlist holds both edits", || {
        note_on(&b).as_deref() == Some(want)
    })
    .await;
}
