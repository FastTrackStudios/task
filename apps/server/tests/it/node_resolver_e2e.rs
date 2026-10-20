#![allow(clippy::large_futures)]
//! `offline.app-node` end to end: an app resolves a node with
//! `task_dial::resolve_node` and dials it — the local node first, with
//! no host anywhere, and the same answer with a host present.
//!
//! ADR 0006 §3 and guardrail 7: an app talks to a node, not to "the
//! server". Two device nodes share the account's org (A made it, B
//! joined it, as `peer_docs_e2e` runs the handshake). The app on A's
//! machine has the account's address book naming both, and no host. It
//! must land on A's own node over loopback and work there; B is its next
//! choice; a host the caller names is the last. Then a host is stood up
//! holding the same org, and the resolver must still pick A's node — the
//! same URL, the same data.

use std::time::Duration;

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use task_dial::{Account, AccountDevice, NodePrefs, Resolved, Via};
use task_server::local_node::{JoinSpec, LOCAL_ENDPOINT_FILE, LocalNode, NodeOptions};
use vox_core::FromVoxLane;

const ORG: &str = "the-band";

fn opts(root: &std::path::Path, peers: bool) -> NodeOptions {
    NodeOptions {
        owner_name: "Sam Device".into(),
        peers,
        ..NodeOptions::at(root)
    }
}

fn client<C: FromVoxLane>(caller: &vox_core::Caller) -> C {
    C::from_vox_lane(caller.clone(), None)
}

/// The node's own person, signed in through `slug`'s auth lane.
async fn sign_in(node: &LocalNode, slug: &str) -> String {
    let owner = node.owner().clone();
    let anon = node.caller(slug, None).await.expect("anonymous lane");
    let auth: AuthServiceClient = client(&anon);
    auth.sign_in_email_password(SignInEmailPassword {
        email: owner.email.clone(),
        password: owner.password.clone(),
        ip_address: None,
        user_agent: Some("node-resolver-e2e".into()),
    })
    .await
    .expect("sign in on the node")
    .token
}

/// Task titles as an app sees them through the resolved node.
async fn titles_via(node: &Resolved, token: &str) -> Vec<String> {
    let tasks: task::TaskServiceClient = task_dial::establish_at(&node.url, Some(token))
        .await
        .unwrap_or_else(|e| panic!("dial the resolved node {}: {e}", node.url));
    let mut titles: Vec<String> = tasks
        .list()
        .await
        .expect("list tasks on the resolved node")
        .into_iter()
        .map(|t| t.title)
        .collect();
    titles.sort();
    titles
}

/// A loopback port nothing is bound to yet.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

async fn eventually_listening(port: u16) {
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the host never listened on {port}");
}

// t[verify offline.app-node] — host absent: two device nodes, the app
// dials its local node; host present: the same node, the same data
#[tokio::test(flavor = "multi_thread")]
async fn an_app_dials_its_local_node_with_no_host_and_the_same_with_one() {
    let env = crate::support::env_lock().await;
    let peer_dir = tempfile::tempdir().unwrap();
    // SAFETY: under the binary's env lock, held while the nodes bind.
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peer_dir.path());
        std::env::set_var("TASK_DEVICE_SYNC_SECS", "0");
    }
    let (a_root, b_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = LocalNode::open(opts(a_root.path(), true))
        .await
        .expect("device A");
    let b = LocalNode::open(opts(b_root.path(), true))
        .await
        .expect("device B");

    // The account's org, made on A and joined by B. The person's account
    // lives in A's home org; that session is the one the app presents on
    // the band's lane too, as `peer_docs_e2e` does.
    let token = sign_in(&a, &a.owner().org.clone()).await;
    let mgmt: org_proto::OrgManagementServiceClient = client(&a.server_caller().await.unwrap());
    let band = mgmt
        .create_org(org_proto::CreateOrgRequest {
            session_token: token.clone(),
            slug: ORG.into(),
            display_name: "The Band".into(),
            is_home: false,
        })
        .await
        .expect("A creates the account's org");
    let a_peer = a.bind(ORG).await.expect("A binds the org");
    let b_peer = b
        .join(JoinSpec {
            slug: ORG.into(),
            id: band.id,
            display_name: band.display_name.clone(),
            peer: Some(a_peer.clone()),
            role: Some("member".into()),
        })
        .await
        .expect("B joins")
        .endpoint_id
        .expect("B is bound for the org");

    // A's desktop serves its node on loopback, as the app does on launch.
    let base = a.serve_loopback().await.expect("A serves on loopback");
    assert!(a_root.path().join(LOCAL_ENDPOINT_FILE).is_file());
    drop(env);

    // The app on A's machine: the account's address book, and no host.
    let account = Account {
        org_slug: ORG.into(),
        org_id: Some(band.id.to_string()),
        this_device: Some("desk".into()),
        devices: vec![
            AccountDevice {
                device_id: "desk".into(),
                endpoint_id: Some(a_peer.clone()),
                last_seen_unix: Some(2),
            },
            AccountDevice {
                device_id: "laptop".into(),
                endpoint_id: Some(b_peer.clone()),
                last_seen_unix: Some(1),
            },
        ],
    };
    let no_host = NodePrefs {
        node_root: Some(a_root.path().to_owned()),
        ..NodePrefs::default()
    };

    // ── host absent ──
    let local = task_dial::resolve_node(&account, &no_host)
        .await
        .expect("a node with no host anywhere");
    assert_eq!(local.via, Via::Local);
    assert_eq!(local.url, format!("{base}/org/{ORG}/vox"));
    assert_eq!(
        task_dial::candidates(&account, &no_host).await,
        vec![
            local.clone(),
            Resolved {
                url: task_dial::peer_url(&b_peer),
                via: Via::Peer {
                    device_id: "laptop".into()
                }
            },
        ],
        "the local node, then the other device, and nothing made up after"
    );

    let tasks: task::TaskServiceClient = task_dial::establish_at(&local.url, Some(&token))
        .await
        .expect("the app dials its local node");
    tasks
        .create(task::capture("Rehearse the bridge"))
        .await
        .expect("the app writes through its local node");
    let absent = titles_via(&local, &token).await;
    assert!(
        absent.iter().any(|t| t == "Rehearse the bridge"),
        "the app's task on its local node: {absent:?}"
    );

    // A peer's address is for an iroh dial, and says so.
    let refused = task_dial::establish_at::<task::TaskServiceClient>(
        &task_dial::peer_url(&b_peer),
        Some(&token),
    )
    .await
    .err()
    .expect("an iroh address is not a WebSocket");
    assert!(refused.contains("iroh transport"), "{refused}");

    // ── host present: a server holding the same org ──
    let h_root = tempfile::tempdir().unwrap();
    let host = LocalNode::open(opts(h_root.path(), false))
        .await
        .expect("the host");
    host.join(JoinSpec {
        slug: ORG.into(),
        id: band.id,
        display_name: band.display_name.clone(),
        peer: Some(a_peer.clone()),
        role: Some("member".into()),
    })
    .await
    .expect("the host holds the org");
    let port = free_port();
    let serving = tokio::spawn(async move {
        let _ = host.serve_at(([127, 0, 0, 1], port).into()).await;
    });
    eventually_listening(port).await;
    let with_host = NodePrefs {
        public_host: Some(format!("http://127.0.0.1:{port}")),
        ..no_host.clone()
    };
    let again = task_dial::resolve_node(&account, &with_host)
        .await
        .expect("a node with the host up");
    assert_eq!(
        again, local,
        "a reachable host changed which node the app dials"
    );
    assert_eq!(
        titles_via(&again, &token).await,
        absent,
        "same node, same data"
    );
    let all = task_dial::candidates(&account, &with_host).await;
    assert_eq!(
        all.last(),
        Some(&Resolved {
            url: format!("ws://127.0.0.1:{port}/org/{ORG}/vox"),
            via: Via::Host
        }),
        "the host is the last resort, not the first: {all:?}"
    );

    // ── the app not running: a stale file falls through to the device ──
    let stale = tempfile::tempdir().unwrap();
    std::fs::write(
        stale.path().join(LOCAL_ENDPOINT_FILE),
        format!("ws://127.0.0.1:{}\n", free_port()),
    )
    .unwrap();
    let gone = task_dial::resolve_node(
        &account,
        &NodePrefs {
            node_root: Some(stale.path().to_owned()),
            ..with_host.clone()
        },
    )
    .await
    .expect("a node with the local one gone");
    assert_eq!(
        gone.via,
        Via::Peer {
            device_id: "laptop".into()
        }
    );
    serving.abort();
    drop(b);
}

// t[verify offline.app-node] — the planted world: a demo person's app
// reaches the example studio through the desktop's node
#[tokio::test(flavor = "multi_thread")]
async fn the_planted_studio_is_reached_through_the_local_node() {
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        plant_example: true,
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the desktop's node with the example studio");
    node.serve_loopback().await.expect("served on loopback");
    let slug = node.owner().org.clone();
    let id = node.manifest(&slug).expect("the planted org").id;

    let resolved = task_dial::resolve_node(
        &Account {
            org_slug: slug.clone(),
            org_id: Some(id.to_string()),
            ..Account::default()
        },
        &NodePrefs {
            node_root: Some(root.path().to_owned()),
            ..NodePrefs::default()
        },
    )
    .await
    .expect("the planted org resolves to the local node");
    assert_eq!(resolved.via, Via::Local);
    let token = sign_in(&node, &slug).await;
    let titles = titles_via(&resolved, &token).await;
    assert!(
        titles.iter().any(|t| t == "Comp lead vocals — Track One"),
        "the seed's tasks through the resolved node: {titles:?}"
    );
}
