//! Live sessions (`live-proto`): a setlist played together, kept open by
//! Task. Two peers join a set and meet in its songs' docs; a doc nobody
//! joined is not served; a playground's epoch moves on and every peer is
//! told; a guest is kept to its link's one set.

#![allow(clippy::large_futures)]

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use collection::{CollectionKind, CollectionService as _, NodeRef, Placement};
use crdt::CrdtDoc;
use crdt::sync::{DocSyncClient, SyncedDoc};
use live_proto::{LiveSessionsClient, LiveSessionsStreamClient};
use task_server::live::{GuestLiveLane, LiveHost, LiveLane, LiveOnly};
use uuid::Uuid;

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// A host over a setlist of two songs; the setlist's id.
fn host(tmp: &std::path::Path) -> (LiveHost, String) {
    let collections = collection::Store::open(tmp.join("collections.jsonl"));
    let set = collections
        .create(
            "live-test".into(),
            "Worship Set".into(),
            CollectionKind::new("songlist"),
        )
        .unwrap();
    for slug in ["washed", "who-else"] {
        collections
            .add_item(Placement {
                collection_id: set.id.clone(),
                node: NodeRef::song(slug),
                after: None,
            })
            .unwrap();
    }
    let resources = resources::ResourcesBackend::new(tmp.join("resources"));
    (
        LiveHost::new("live-test".into(), collections, resources),
        set.id,
    )
}

/// A member's router: the lanes the org mounts for live sessions.
fn member(host: &LiveHost) -> LocalServer {
    let only = LiveOnly(host.clone());
    let router = LayerRouter::new()
        .with(
            live_proto::live_sessions_rpc_service_descriptor(),
            live_proto::serve(LiveLane(host.clone())),
        )
        .merge(live_proto::stream_layer(LiveLane(host.clone())))
        .with(
            crdt::sync::doc_sync_service_descriptor(),
            crdt::sync::DocSyncDispatcher::new(only),
        );
    LocalServer::serve(router, Scope::new())
}

/// A replica of `doc_id`, syncing.
async fn replica(local: &LocalServer, doc_id: Uuid) -> CrdtDoc {
    let doc = CrdtDoc::ephemeral();
    let mut synced = SyncedDoc::new(doc_id, doc.clone());
    let client: DocSyncClient = local.establish().await.expect("DocSyncClient");
    tokio::spawn(async move {
        let _ = synced.run(&client).await;
    });
    doc
}

fn tempo(doc: &CrdtDoc) -> Option<String> {
    doc.loro()
        .get_map("song")
        .get("tempo")
        .and_then(|v| v.into_value().ok())
        .and_then(|v| v.into_string().ok())
        .map(|s| s.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn two_peers_meet_in_a_songs_doc_through_task() {
    let tmp = tempfile::tempdir().unwrap();
    let (host, setlist) = host(tmp.path());
    let local = member(&host);
    let live: LiveSessionsClient = local.establish().await.unwrap();

    let set = live.join(setlist.clone()).await.unwrap();
    assert_eq!(set.title, "Worship Set");
    assert_eq!(set.epoch, 0);
    assert_eq!(set.resets_at, None, "a set that keeps what is done in it");
    let slugs: Vec<&str> = set.songs.iter().map(|s| s.slug.as_str()).collect();
    assert_eq!(slugs, ["washed", "who-else"], "the set's songs, in order");
    assert!(
        set.songs.iter().all(|s| s.files.is_none()),
        "a member reads the library itself"
    );
    let before = live.now().await.unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert!(live.now().await.unwrap() > before, "Task's clock moves on");

    // The first peer seeds a song's doc; the second sees it.
    let washed: Uuid = set.songs[0].doc_id.parse().unwrap();
    let first = replica(&local, washed).await;
    let second = replica(&local, washed).await;
    first.loro().get_map("song").insert("tempo", "78").unwrap();
    first.loro().commit();
    eventually("the second peer has the first's edit", || {
        tempo(&second).as_deref() == Some("78")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_doc_nobody_joined_is_not_served() {
    let tmp = tempfile::tempdir().unwrap();
    let (host, setlist) = host(tmp.path());
    assert!(
        !host.admits(task_server::live::doc_id(
            "live-test",
            &setlist,
            0,
            "washed"
        )),
        "not before a join"
    );
    host.join(&setlist, None).unwrap();
    assert!(host.admits(task_server::live::doc_id(
        "live-test",
        &setlist,
        0,
        "washed"
    )));
    assert!(!host.admits(Uuid::new_v4()), "never a stranger's id");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_playgrounds_epoch_moves_on_and_every_peer_is_told() {
    let tmp = tempfile::tempdir().unwrap();
    let (host, setlist) = host(tmp.path());
    let local = member(&host);
    let live: LiveSessionsClient = local.establish().await.unwrap();
    let stream: LiveSessionsStreamClient = local.establish().await.unwrap();
    let set = live.join(setlist.clone()).await.unwrap();
    let old: Uuid = set.songs[0].doc_id.parse().unwrap();
    let (tx, mut rx) = vox::channel::<live_proto::LiveEpoch>();
    let _sub = tokio::spawn(async move {
        let _ = stream.epochs(tx).await;
    });
    // Let the subscription attach: the hub replays nothing.
    eventually("the subscription attached", || {
        host.epochs_hub().subscriber_count() > 0
    })
    .await;

    assert_eq!(host.next_epoch(&setlist), Some(1));
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("told in time")
        .expect("rx error")
        .expect("rx open");
    let mut told = None;
    let _ = msg.map(|e| told = Some(e.clone()));
    let told = told.expect("an epoch");
    assert_eq!((told.setlist.as_str(), told.epoch), (setlist.as_str(), 1));
    assert!(
        !host.admits(old),
        "the old epoch's docs are no longer served"
    );

    let again = live.join(setlist.clone()).await.unwrap();
    assert_eq!(again.epoch, 1);
    assert_ne!(
        again.songs[0].doc_id, set.songs[0].doc_id,
        "the song starts over under a new doc"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_is_kept_to_its_links_set_and_given_its_songs_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (host, setlist) = host(tmp.path());
    let files = std::collections::HashMap::from([(
        "washed".to_owned(),
        "https://task.test/org/x/share/w".to_owned(),
    )]);
    let lane = GuestLiveLane {
        host: host.clone(),
        setlist: setlist.clone(),
        reset: Some(Duration::from_secs(300)),
        files: std::sync::Arc::new(files),
    };
    let router = LayerRouter::new().with(
        live_proto::live_sessions_rpc_service_descriptor(),
        live_proto::serve(lane),
    );
    let local = LocalServer::serve(router, Scope::new());
    let live: LiveSessionsClient = local.establish().await.unwrap();

    let set = live.join(setlist.clone()).await.unwrap();
    assert_eq!(set.resets_every_secs, Some(300), "a playground");
    let (now, resets_at) = (
        live.now().await.unwrap(),
        set.resets_at.expect("a playground ends"),
    );
    assert!(
        resets_at > now && resets_at <= now + 300e6,
        "its run ends within the interval, on Task's clock"
    );
    assert_eq!(
        set.songs[0].files.as_deref(),
        Some("https://task.test/org/x/share/w")
    );
    assert_eq!(
        set.songs[1].files, None,
        "a song with no session in Task has no files"
    );
    assert!(
        live.join("another-set".into()).await.is_err(),
        "only the link's set"
    );
    assert_eq!(
        live.join(String::new()).await.unwrap().setlist,
        setlist,
        "empty is the link's set"
    );
}
