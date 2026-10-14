#![allow(clippy::large_futures)]
//! The device's own node (`offline.local`, `offline.peers`): the app's
//! backend booted on a temp data root, driven through the SAME client
//! calls the UI makes — and no network listener anywhere. No TCP bind,
//! no HTTP, no WebSocket: discovery is the node's own document, every
//! lane an in-process caller with the session baked into the gate.
//!
//! 1. [`a_fresh_device_is_fully_functional_with_no_server`] — first run
//!    provisions a personal org and its owner; the owner signs in over
//!    the org's own `AuthService`; then task create/update, an inbox
//!    capture processed into a task, the Suggested pick over what the
//!    node returned, and the activity feed recording all of it; a reboot
//!    keeps the world and the person.
//! 2. [`a_second_device_joins_the_org_as_a_node`] — two nodes, no host:
//!    the second takes a node of an org the first holds (same slug, same
//!    id), admits it, and the first enrolls the second through its own
//!    `SyncService` — the join handshake a device runs against a host.
//!    A different org under a slug already held is refused.

use std::time::Duration;

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use notify_proto::{Activity, ActivityFeedClient, ActivityFilter, ActivityKind};
use task_server::local_node::{JoinSpec, LocalNode, NodeOptions, PERSONAL_SLUG};
use vox_core::FromVoxLane;

fn opts(root: &std::path::Path, peers: bool) -> NodeOptions {
    NodeOptions {
        owner_name: "Sam Device".into(),
        peers,
        ..NodeOptions::at(root)
    }
}

/// A typed view over a raw caller — what `establish_for` does in the UI.
fn client<C: FromVoxLane>(caller: &vox_core::Caller) -> C {
    C::from_vox_lane(caller.clone(), None)
}

/// Sign the node's own person in, the way the app does at boot.
async fn sign_in(node: &LocalNode) -> String {
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.expect("anonymous lane");
    let auth: AuthServiceClient = client(&anon);
    auth.sign_in_email_password(SignInEmailPassword {
        email: owner.email.clone(),
        password: owner.password.clone(),
        ip_address: None,
        user_agent: Some("local-node-e2e".into()),
    })
    .await
    .expect("the device's person signs in on its own node")
    .token
}

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
            "activity never arrived; have: {:?}",
            rows.iter().map(|a| a.kind).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// t[verify offline.local]
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_device_is_fully_functional_with_no_server() {
    let _env = crate::support::env_lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let node = LocalNode::open(opts(tmp.path(), false))
        .await
        .expect("a fresh device boots its node");

    // First run made a personal org, home, owned by the device's person.
    assert_eq!(node.org_slugs(), vec![PERSONAL_SLUG.to_owned()]);
    let owner = node.owner().clone();
    assert_eq!(owner.org, PERSONAL_SLUG);
    assert_eq!(owner.name, "Sam Device");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let creds = tmp.path().join(task_server::local_node::OWNER_FILE);
        let mode = std::fs::metadata(&creds).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the owner's credentials are private");
    }

    // Discovery, with no HTTP: anonymous lists the org; with the session
    // it names the principal — the shape the app's parser reads.
    let anon = node.discovery(None).await;
    assert_eq!(anon["orgs"][0]["slug"], PERSONAL_SLUG);
    assert_eq!(anon["orgs"][0]["is_home"], true);
    let token = sign_in(&node).await;
    let signed = node.discovery(Some(token.clone())).await;
    assert_eq!(signed["principal"]["email"], owner.email.as_str());
    assert_eq!(signed["orgs"][0]["member"], true);

    // Every client below is a view over ONE in-process caller holding
    // the session — what `caller_for` hands the UI.
    let lane = node
        .caller(PERSONAL_SLUG, Some(token.clone()))
        .await
        .expect("signed-in lane");
    let auth: AuthServiceClient = client(&lane);
    let me = auth
        .whoami(token.clone())
        .await
        .expect("whoami on the node");
    assert_eq!(me.email.as_deref(), Some(owner.email.as_str()));
    assert_eq!(
        me.role.as_deref(),
        Some("admin"),
        "the device's person owns it"
    );

    let tasks: task::TaskServiceClient = client(&lane);
    let inbox: inbox_proto::InboxClient = client(&lane);
    let feed: ActivityFeedClient = client(&lane);

    // ── a task, created and moved ──
    let mut draft = task::capture("Comp lead vocals");
    draft.status = "open".into();
    let created = tasks.create(draft).await.expect("create");
    let mut moved = created.clone();
    moved.status = "in-progress".into();
    tasks.update(moved).await.expect("update");
    let listed = tasks.list().await.expect("list");
    let back = listed.iter().find(|t| t.id == created.id).expect("listed");
    assert_eq!(back.status, "in-progress");

    // ── a capture, processed into a task ──
    let capture = inbox_proto::InboxItem::capture(
        uuid::Uuid::new_v4().to_string(),
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
    let items = inbox.list_inbox().await.expect("list inbox");
    let item = items.iter().find(|i| i.id == capture.id).expect("kept");
    assert_eq!(item.status, inbox_proto::InboxItem::STATUS_PROCESSED);

    // ── Suggested, over what the node answered (the Tasks page's slot) ──
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mine: Vec<&task::TaskInfo> = listed.iter().collect();
    let to_process = items.iter().filter(|i| i.is_due(&today)).count();
    let picks = task::suggest::candidates(&task::suggest::SuggestInput {
        now: None,
        mine: &mine,
        timing: None,
        to_process,
        today: &today,
        declined: &[],
    });
    assert!(
        picks.iter().any(|s| matches!(
            s,
            task::suggest::Suggestion::Continue(t) if t.id == created.id
        )),
        "the task already moving is suggested: {picks:?}"
    );

    // ── the activity feed recorded it all ──
    let rows = wait_for(&feed, ActivityFilter::entity("task", created.id), |r| {
        r.iter().any(|a| a.kind == ActivityKind::StatusChanged)
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
    wait_for(&feed, ActivityFilter::entity("inbox", &capture.id), |r| {
        r.iter().any(|a| a.kind == ActivityKind::Processed)
    })
    .await;

    // A reboot of the device keeps its world and its person.
    drop((feed, tasks, inbox, auth, lane));
    drop(node);
    let again = LocalNode::open(opts(tmp.path(), false))
        .await
        .expect("reboot");
    assert_eq!(again.owner(), &owner);
    let token = sign_in(&again).await;
    let lane = again.caller(PERSONAL_SLUG, Some(token)).await.unwrap();
    let tasks: task::TaskServiceClient = client(&lane);
    assert!(
        tasks
            .list()
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == created.id)
    );
}

// t[verify offline.local] — a first run cut short (the app killed while
// it planted) is started again, not a device that never boots; a data
// root with orgs nobody provisioned here is still refused
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_first_run_provisions_again() {
    let _env = crate::support::env_lock().await;
    let tmp = tempfile::tempdir().unwrap();
    // What a killed first run leaves: the marker, an org, no owner.
    let data_root = org_proto::DataRoot::new(tmp.path().to_path_buf());
    data_root.ensure().unwrap();
    data_root.init_org("half-made", "Half made", true).unwrap();
    std::fs::write(
        tmp.path().join(task_server::local_node::PROVISIONING_FILE),
        b"",
    )
    .unwrap();

    let node = LocalNode::open(opts(tmp.path(), false))
        .await
        .expect("the interrupted first run is redone");
    assert_eq!(node.org_slugs(), vec![PERSONAL_SLUG.to_owned()]);
    assert!(
        !tmp.path()
            .join(task_server::local_node::PROVISIONING_FILE)
            .exists(),
        "the first run finished"
    );
    drop(node);

    // Orgs without the marker are somebody else's: refused, untouched.
    let other = tempfile::tempdir().unwrap();
    let theirs = org_proto::DataRoot::new(other.path().to_path_buf());
    theirs.ensure().unwrap();
    theirs.init_org("theirs", "Theirs", true).unwrap();
    assert!(LocalNode::open(opts(other.path(), false)).await.is_err());
    assert!(theirs.org("theirs").path().is_dir());
}

// t[verify offline.peers]
#[tokio::test(flavor = "multi_thread")]
async fn a_second_device_joins_the_org_as_a_node() {
    let _env = crate::support::env_lock().await;
    let (a_root, b_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = LocalNode::open(opts(a_root.path(), true))
        .await
        .expect("device A");
    let b = LocalNode::open(opts(b_root.path(), true))
        .await
        .expect("device B");
    // Both are nodes of their own orgs from the first boot: bound,
    // dialable by id, and not the same id.
    let a_home = a.endpoint_id(PERSONAL_SLUG).expect("A is bound");
    let b_home = b.endpoint_id(PERSONAL_SLUG).expect("B is bound");
    assert_ne!(a_home, b_home);

    // A makes an org for the band over its own /server lane, signed in
    // as its person — the host's rule (home-org users mint orgs), on a
    // device.
    let a_token = sign_in(&a).await;
    let mgmt: org_proto::OrgManagementServiceClient = client(&a.server_caller().await.unwrap());
    let band = mgmt
        .create_org(org_proto::CreateOrgRequest {
            session_token: a_token.clone(),
            slug: "the-band".into(),
            display_name: "The Band".into(),
            is_home: false,
        })
        .await
        .expect("A creates the band's org");
    assert!(a.hosts("the-band"));
    // An org that arrived after boot is made dialable like the rest.
    let a_band = a
        .bind("the-band")
        .await
        .expect("A binds the band's endpoint");

    // B takes a node of it: the org's identity, and A's endpoint for it
    // (A's `coordinator()` — what the app asks a host for).
    let a_lane = a.caller("the-band", Some(a_token.clone())).await.unwrap();
    let a_sync: files_proto::service::sync::SyncServiceClient = client(&a_lane);
    let a_peer = a_sync
        .coordinator()
        .await
        .expect("A's endpoint for the band");
    assert_eq!(
        a_peer, a_band,
        "coordinator() names the endpoint just bound"
    );
    let joined = b
        .join(JoinSpec {
            slug: "the-band".into(),
            id: band.id,
            display_name: band.display_name.clone(),
            peer: Some(a_peer.clone()),
            role: Some("member".into()),
        })
        .await
        .expect("device B joins");
    assert!(b.hosts("the-band"));
    let b_org = b.state().data_root.org("the-band");
    assert_eq!(
        b_org.manifest().unwrap().id,
        band.id,
        "the same org, not a namesake"
    );
    let admitted = std::fs::read_to_string(b_org.path().join("admitted-devices")).unwrap();
    assert!(admitted.lines().any(|l| l.trim() == a_peer), "B admits A");
    let b_peer = joined.endpoint_id.expect("B is bound for the band");
    assert_ne!(b_peer, a_peer);

    // A enrolls B — the other half, exactly as the app does on a host.
    a_sync
        .enroll_device(b_peer.clone(), "device B".into())
        .await
        .expect("A admits B");

    // B's person reaches the joined org on B's own node through B's home
    // identity — signed in once, at home, no host involved.
    let b_token = sign_in(&b).await;
    let b_tasks: task::TaskServiceClient =
        client(&b.caller("the-band", Some(b_token)).await.unwrap());
    b_tasks
        .create(task::capture("Rehearse the bridge"))
        .await
        .expect("B works in the joined org with no host");

    // Idempotent: joining again changes nothing.
    let again = b
        .join(JoinSpec {
            slug: "the-band".into(),
            id: band.id,
            display_name: band.display_name.clone(),
            peer: Some(a_peer.clone()),
            role: Some("member".into()),
        })
        .await
        .unwrap();
    assert_eq!(again.endpoint_id.as_deref(), Some(b_peer.as_str()));
    let admitted = std::fs::read_to_string(b_org.path().join("admitted-devices")).unwrap();
    assert_eq!(admitted.lines().filter(|l| l.trim() == a_peer).count(), 1);

    // A DIFFERENT org under a slug B already holds is refused, not merged:
    // A's personal org is not B's.
    let a_personal = a
        .state()
        .data_root
        .org(PERSONAL_SLUG)
        .manifest()
        .unwrap()
        .id;
    let refused = b
        .join(JoinSpec {
            slug: PERSONAL_SLUG.into(),
            id: a_personal,
            display_name: "A's".into(),
            peer: Some(a_home),
            role: None,
        })
        .await;
    assert!(refused.is_err(), "a namesake org is refused");
}
