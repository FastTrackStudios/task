// The setlist half exercises the `fasttrackstudio` plugin's collection
// lane; a build without it has nothing to cover.
#![cfg(feature = "plugin-fasttrackstudio")]
#![allow(clippy::large_futures)]
//! Safe saves with **no host**: two device nodes, each with its own
//! client, and the late save is `Stale` rather than an overwrite.
//!
//! The integration chapter (`tests/integration/tests/it/safe_saves.rs`)
//! proves the rule against a server. ADR 0006 says a feature that syncs
//! or serves must also hold with the server gone, and this is that stage:
//! the band's org lives on two people's machines (A made it, B joined
//! it), nothing else is running, and the sweeps between the two nodes
//! are what carry the documents (`device_sync::sweep`, as `peer_docs_e2e`
//! drives it).
//!
//! - A **chart** and a **song** written on A reach B. A client on each
//!   node reads them and gets the *same* revision — although B's
//!   collaborative layer writes its copy back with the frontmatter in
//!   another order, a revision digests what the document says, not its
//!   bytes, so it means one thing on every node. B's client
//!   saves first; the sweep brings that to A; A's client, still holding
//!   the old revision, saves and is told `Stale`, and B's edit is what A
//!   holds.
//! - A **setlist** lives in one node's collection store (collections do
//!   not travel between devices), so its two clients are two sessions on
//!   A — a laptop app and the CLI, say — with the host still absent. The
//!   late whole-list save is `Stale`.

use std::time::Duration;

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use collection_proto::{CollectionError, CollectionKind, CollectionSave, CollectionServiceClient};
use links_proto::NodeRef;
use resources_proto::{ChartDoc, ResourcesError, ResourcesServiceClient, SongDoc};
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
        user_agent: Some("safe-saves-e2e".into()),
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

async fn eventually_at<T>(what: &str, mut read: impl AsyncFnMut() -> Option<T>) -> T {
    for _ in 0..300 {
        if let Some(found) = read().await {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// The chart, once `res`'s node holds it at revision `want`.
async fn chart_at(res: &ResourcesServiceClient, slug: &str, want: &str, what: &str) -> ChartDoc {
    eventually_at(what, async || {
        res.chart(slug.to_owned())
            .await
            .ok()
            .filter(|c| c.revision == want)
    })
    .await
}

/// The song, once `res`'s node holds it at revision `want`.
async fn song_at(res: &ResourcesServiceClient, slug: &str, want: &str, what: &str) -> SongDoc {
    eventually_at(what, async || {
        res.song(slug.to_owned())
            .await
            .ok()
            .filter(|s| s.revision == want)
    })
    .await
}

fn resources_err(e: architect::vox::VoxError<ResourcesError>) -> ResourcesError {
    match e {
        architect::vox::VoxError::User(e) => *e,
        other => ResourcesError::Io(other.to_string()),
    }
}

fn collection_err(e: architect::vox::VoxError<CollectionError>) -> CollectionError {
    match e {
        architect::vox::VoxError::User(e) => *e,
        other => CollectionError::Io(other.to_string()),
    }
}

// t[verify storage.save.revision] — host absent: a chart and a song
// edited from two device nodes; the late save is Stale on its node
// t[verify storage.save.collection] — host absent: two clients on a
// device node, one setlist; the late whole-list save is Stale
#[tokio::test(flavor = "multi_thread")]
async fn the_late_save_is_stale_with_no_host() {
    let env = crate::support::env_lock().await;
    let peer_dir = tempfile::tempdir().unwrap();
    // SAFETY: under the binary's env lock, held while both nodes bind.
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peer_dir.path());
        // Sweeps run when this test says, not on a timer.
        std::env::set_var("TASK_DEVICE_SYNC_SECS", "0");
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
    // other — the join handshake, as `peer_docs_e2e` runs it.
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

    let b_lane = b.caller(ORG, Some(sign_in(&b).await)).await.unwrap();
    let a_res: ResourcesServiceClient = client(&a_lane);
    let b_res: ResourcesServiceClient = client(&b_lane);

    // ── A writes a song and its chart; the sweeps carry them to B ──
    let song = a_res
        .upsert_song(SongDoc {
            title: "Opener".into(),
            key: "G".into(),
            ..SongDoc::default()
        })
        .await
        .expect("A writes the song");
    let chart = a_res
        .upsert_chart(ChartDoc {
            title: "Opener".into(),
            source: "[Verse]\n| G | C | G | D |\n".into(),
            song: format!("song:{}", song.slug),
            ..ChartDoc::default()
        })
        .await
        .expect("A writes the chart");

    // ── the sweeps carry them to B; one client on each node reads
    // both, and gets one revision each ──
    sweep(&b).await;
    sweep(&a).await;
    let a_chart = a_res.chart(chart.slug.clone()).await.expect("A reads");
    let a_song = a_res.song(song.slug.clone()).await.expect("A reads");
    assert!(!a_chart.revision.is_empty());
    // B's copy is written back by its collaborative layer, which orders
    // the frontmatter its own way: byte-different, the same document —
    // and so, by construction, the same revision.
    let b_chart = chart_at(&b_res, &chart.slug, &a_chart.revision, "the chart on B").await;
    let b_song = song_at(&b_res, &song.slug, &a_song.revision, "the song on B").await;
    assert_eq!(
        a_chart.revision, b_chart.revision,
        "one document, one revision, on every node"
    );
    assert_eq!(a_song.revision, b_song.revision);

    // ── B saves first ──
    let b_source = "[Verse]\n| G | C | Em | D |\n".to_owned();
    let b_saved = b_res
        .upsert_chart(ChartDoc {
            source: b_source.clone(),
            ..b_chart.clone()
        })
        .await
        .expect("B's chart save lands");
    let b_song_saved = b_res
        .upsert_song(SongDoc {
            key: "A".into(),
            ..b_song.clone()
        })
        .await
        .expect("B's song save lands");

    // ── the sweeps bring B's saves to A ──
    tokio::time::sleep(Duration::from_millis(300)).await;
    sweep(&a).await;
    sweep(&b).await;
    chart_at(&a_res, &chart.slug, &b_saved.revision, "B's chart on A").await;
    song_at(&a_res, &song.slug, &b_song_saved.revision, "B's song on A").await;

    // ── A's client, still on the old revisions, saves: Stale ──
    let late = a_res
        .upsert_chart(ChartDoc {
            source: "[Verse]\n| G | D | G | D |\n".into(),
            ..a_chart.clone()
        })
        .await
        .map_err(resources_err)
        .expect_err("A's chart save was made against an old revision");
    assert_eq!(
        late,
        ResourcesError::Stale(b_saved.revision.clone()),
        "Stale, at the revision B's save made — the same string on A"
    );
    let late_song = a_res
        .upsert_song(SongDoc {
            key: "C".into(),
            ..a_song.clone()
        })
        .await
        .map_err(resources_err)
        .expect_err("A's song save was made against an old revision");
    assert_eq!(
        late_song,
        ResourcesError::Stale(b_song_saved.revision.clone())
    );

    // B's edits are what A holds; the refusals wrote nothing.
    let a_now = a_res.chart(chart.slug.clone()).await.expect("A re-reads");
    assert_eq!(a_now.source, b_source, "B's chart edit survived on A");
    assert_eq!(a_now.revision, b_saved.revision);
    assert_eq!(
        a_res.song(song.slug.clone()).await.expect("A re-reads").key,
        "A",
        "B's song edit survived on A"
    );
    // And A, holding the current revision, saves.
    a_res
        .upsert_chart(ChartDoc {
            source: "[Verse]\n| G | C | Em | D |\n[Tag]\n| D |\n".into(),
            ..a_now
        })
        .await
        .expect("a save against the current revision lands on A");

    // ── a setlist: two clients on A, the host still absent ──
    let laptop: CollectionServiceClient = client(&a_lane);
    let cli: CollectionServiceClient =
        client(&a.caller(ORG, Some(sign_in(&a).await)).await.unwrap());
    let set = laptop
        .create(
            ORG.into(),
            "Friday Set".into(),
            CollectionKind::new("setlist"),
        )
        .await
        .expect("a setlist on A");
    for node in [
        NodeRef::song(&song.slug),
        NodeRef::parse(&format!("chart:{}", chart.slug)).expect("a chart token"),
    ] {
        laptop
            .add_item(collection_proto::Placement {
                collection_id: set.id.clone(),
                node,
                after: None,
            })
            .await
            .expect("add to the set");
    }
    let on_laptop = laptop.get(set.id.clone()).await.unwrap().unwrap();
    let on_cli = cli.get(set.id.clone()).await.unwrap().unwrap();
    assert_eq!(on_laptop.revision, on_cli.revision);
    let order: Vec<NodeRef> = on_cli.items.iter().map(|it| it.node.clone()).collect();

    let first = cli
        .save(CollectionSave {
            id: set.id.clone(),
            revision: on_cli.revision,
            title: "Friday Set".into(),
            items: order.iter().rev().cloned().collect(),
        })
        .await
        .expect("the CLI's save lands");
    let late_set = laptop
        .save(CollectionSave {
            id: set.id.clone(),
            revision: on_laptop.revision,
            title: "Friday Late Set".into(),
            items: order.clone(),
        })
        .await
        .map_err(collection_err)
        .expect_err("the laptop's list was made against an old revision");
    assert_eq!(late_set, CollectionError::Stale(first.revision));
    let held = laptop.get(set.id.clone()).await.unwrap().unwrap();
    assert_eq!(held.title, "Friday Set", "the refused save wrote nothing");
    assert_eq!(
        held.items
            .iter()
            .map(|it| it.node.clone())
            .collect::<Vec<_>>(),
        order.into_iter().rev().collect::<Vec<_>>(),
        "the early save survived"
    );
}
