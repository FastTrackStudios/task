#![allow(clippy::large_futures)]
//! An agent in a live set over MCP (spec §7 S3): `live_observe` a set,
//! `doc_read` a song's doc as JSON with its version, `doc_apply` ops to
//! it — applied at once (§11 Q2), so a peer syncing the doc sees the
//! edit live; refused when stale or when it breaks the doc's shape;
//! attributed to the caller in the CRDT history; undone by its apply id.
//! A key that only observes is refused the edit.
//!
//! One node, its person signed in, through the real transports: MCP over
//! HTTP for the agents, the org's vox lane (`LiveSessions` + `DocSync`)
//! for the band's peer.

use std::time::Duration;

use architect_auth::SignInEmailPassword;
use architect_auth::proto::AuthServiceClient;
use collection::{CollectionKind, CollectionService as _, NodeRef, Placement};
use crdt::CrdtDoc;
use crdt::sync::{DocSyncClient, SyncedDoc};
use live_proto::LiveSessionsClient;
use serde_json::{Value, json};
use task_server::bots::{self, OrgChoice, Tools};
use task_server::local_node::{LocalNode, NodeOptions, PERSONAL_SLUG};
use uuid::Uuid;
use vox_core::FromVoxLane;

/// One tool call: whether it failed, and its payload (JSON when it is).
async fn tool(
    http: &reqwest::Client,
    url: &str,
    key: &str,
    name: &str,
    args: Value,
) -> (bool, Value) {
    let res: Value = http
        .post(url)
        .bearer_auth(key)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": args },
        }))
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("a JSON-RPC body");
    let result = &res["result"];
    assert!(!result.is_null(), "{name} answered {res}");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    (
        result["isError"].as_bool().unwrap_or(false),
        serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
    )
}

/// A call that must succeed.
async fn ok(http: &reqwest::Client, url: &str, key: &str, name: &str, args: Value) -> Value {
    let (err, v) = tool(http, url, key, name, args.clone()).await;
    assert!(!err, "{name}({args}) failed: {v}");
    v
}

/// A call that must be refused; its message.
async fn refused(http: &reqwest::Client, url: &str, key: &str, name: &str, args: Value) -> String {
    let (err, v) = tool(http, url, key, name, args.clone()).await;
    assert!(err, "{name}({args}) should be refused, answered {v}");
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..400 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// A replica of `doc_id`, syncing over the org's vox socket.
async fn replica(vox_url: &str, doc_id: Uuid) -> CrdtDoc {
    let doc = CrdtDoc::ephemeral();
    let mut synced = SyncedDoc::new(doc_id, doc.clone());
    let client: DocSyncClient = vox::connect_lane(vox_url).establish().await.unwrap();
    tokio::spawn(async move {
        let _ = synced.run(&client).await;
    });
    doc
}

/// `song.<name>` of a replica, as JSON (a list or map deeply).
fn field(doc: &CrdtDoc, name: &str) -> Value {
    doc.loro()
        .get_map("song")
        .get(name)
        .map_or(Value::Null, |v| {
            task_server::live_docs::to_json(&v.get_deep_value())
        })
}

// t[verify mcp.live.observe]
// t[verify mcp.live.apply]
// t[verify mcp.live.expected-version]
// t[verify mcp.live.attribution]
// t[verify mcp.live.undo]
// t[verify mcp.live.observer]
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_edits_a_live_song_that_a_peer_sees_and_undoes_it() {
    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Cody".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");
    let base = node.serve_loopback().await.unwrap();
    let url = format!(
        "{}/org/{PERSONAL_SLUG}/mcp",
        base.replacen("ws://", "http://", 1)
    );
    let http = reqwest::Client::new();

    // The person, signed in and admitted to their org.
    let owner = node.owner().clone();
    let anon = node.caller(&owner.org, None).await.unwrap();
    let session = AuthServiceClient::from_vox_lane(anon, None)
        .sign_in_email_password(SignInEmailPassword {
            email: owner.email.clone(),
            password: owner.password.clone(),
            ip_address: None,
            user_agent: Some("live-docs-e2e".into()),
        })
        .await
        .expect("the person signs in");
    let me = session.user.id;
    let token = session.token.clone();
    node.admit_account(me, &[PERSONAL_SLUG.to_owned()], "admin")
        .await
        .unwrap();

    // Atlas, an assistant with every tool; Screen, a key that only watches.
    let home = node
        .state()
        .home_identity
        .clone()
        .expect("a node has a home org");
    let make = |name: &'static str, tools: Tools| {
        let home = home.clone();
        async move {
            bots::create(
                &home.auth,
                &home.memberships,
                me,
                name,
                &OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]),
                tools,
                bots::Kind::Team,
            )
            .await
            .expect("the bot is made")
        }
    };
    let atlas = make("Atlas", Tools::All).await;
    let screen = make("Screen", Tools::Library).await;
    let atlas_ref = format!("bot:{}", atlas.bot.id);

    // A set of two songs in the org.
    let org = node.state().org(PERSONAL_SLUG).expect("the personal org");
    let set = org
        .collections
        .create(
            PERSONAL_SLUG.into(),
            "Worship Set".into(),
            CollectionKind::new("songlist"),
        )
        .unwrap();
    for slug in ["washed", "who-else"] {
        org.collections
            .add_item(Placement {
                collection_id: set.id.clone(),
                node: NodeRef::song(slug),
                after: None,
            })
            .unwrap();
    }
    let setlist = set.id.clone();

    // ── The band's peer joins over the org's vox socket and seeds the song ──
    let vox_url = format!("{base}/org/{PERSONAL_SLUG}/vox");
    let live: LiveSessionsClient = vox::connect_lane(&vox_url).establish().await.unwrap();
    let joined = live.join(setlist.clone()).await.expect("the peer joins");
    let washed: Uuid = joined.songs[0].doc_id.parse().unwrap();
    let peer = replica(&vox_url, washed).await;
    let song = peer.loro().get_map("song");
    song.insert("tempo", 78).unwrap();
    song.insert("key", "G").unwrap();
    peer.loro().commit();
    // A second peer has the seed: the set syncs both ways.
    let second = replica(&vox_url, washed).await;
    eventually("a second peer has the first's seed", || {
        field(&second, "tempo") == json!(78)
    })
    .await;

    // ── live_observe: the set, each song's doc ref and version ──
    let watched = ok(
        &http,
        &url,
        &atlas.key,
        "live_observe",
        json!({ "setlist": setlist }),
    )
    .await;
    assert_eq!(
        watched["read_only"],
        json!(false),
        "Atlas may edit: {watched}"
    );
    let doc_ref = format!("live:{setlist}/washed");
    assert_eq!(watched["songs"][0]["doc_ref"], json!(doc_ref), "{watched}");
    assert_eq!(watched["songs"][0]["doc_id"], json!(washed.to_string()));
    assert_eq!(watched["songs"][1]["slug"], json!("who-else"));

    // ── doc_read: JSON and a version, once the peer's seed has arrived ──
    let mut read = Value::Null;
    for _ in 0..400 {
        read = ok(
            &http,
            &url,
            &atlas.key,
            "doc_read",
            json!({ "doc_ref": doc_ref }),
        )
        .await;
        if read["value"]["song"]["tempo"] == json!(78) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        read["value"]["song"],
        json!({ "key": "G", "tempo": 78 }),
        "{read}"
    );
    let v1 = read["version"].as_str().expect("a version").to_owned();
    assert_ne!(v1, "empty");

    // ── doc_apply: applied by default, visible to the live peer ──
    let applied = ok(
        &http,
        &url,
        &atlas.key,
        "doc_apply",
        json!({
            "doc_ref": doc_ref,
            "expected_version": v1,
            // An agent's argument can't name someone else.
            "created_by": format!("user:{me}"),
            "ops": [
                { "op": "set", "path": ["song", "tempo"], "value": 82 },
                { "op": "set", "path": ["song", "sections"], "value": ["verse", "chorus"] },
            ],
        }),
    )
    .await;
    assert_eq!(applied["applied"], json!(true), "{applied}");
    assert_eq!(applied["attribution"]["created_by"], json!(atlas_ref));
    let apply_id = applied["apply"].as_str().expect("an apply id").to_owned();
    let v2 = applied["version"].as_str().unwrap().to_owned();
    assert_ne!(v2, v1, "the edit moved the version");
    eventually("the other peer sees it too", || {
        field(&second, "tempo") == json!(82)
    })
    .await;
    eventually("the peer sees the agent's edit live", || {
        field(&peer, "tempo") == json!(82) && field(&peer, "sections") == json!(["verse", "chorus"])
    })
    .await;

    // The CRDT history the peer holds names who made it.
    let (counter, peer_id) = v2.split_once('@').expect("counter@peer");
    let change = peer
        .loro()
        .get_change(crdt::loro::ID::new(
            peer_id.parse().unwrap(),
            counter.parse().unwrap(),
        ))
        .expect("the peer holds the apply's change");
    let message: Value =
        serde_json::from_str(change.message.as_deref().expect("a commit message")).unwrap();
    assert_eq!(message["created_by"], json!(atlas_ref), "{message}");
    assert_eq!(message["apply"], json!(apply_id));

    // ── A stale expected_version is refused, and nothing changes ──
    let stale = refused(
        &http,
        &url,
        &atlas.key,
        "doc_apply",
        json!({
            "doc_ref": doc_ref,
            "expected_version": v1,
            "ops": [{ "op": "set", "path": ["song", "tempo"], "value": 60 }],
        }),
    )
    .await;
    assert!(stale.contains("stale"), "{stale}");
    // …and so is an op that breaks the doc's shape, the whole batch with it.
    let schema = refused(
        &http,
        &url,
        &atlas.key,
        "doc_apply",
        json!({
            "doc_ref": doc_ref,
            "ops": [
                { "op": "set", "path": ["song", "key"], "value": "A" },
                { "op": "set", "path": ["song", "tempo"], "value": "fast" },
            ],
        }),
    )
    .await;
    assert!(
        schema.contains("schema") && schema.contains("op 1"),
        "{schema}"
    );
    let now = ok(
        &http,
        &url,
        &atlas.key,
        "doc_read",
        json!({ "doc_ref": doc_ref }),
    )
    .await;
    assert_eq!(now["version"], json!(v2), "neither refusal touched the doc");
    assert_eq!(now["value"]["song"]["key"], json!("G"));

    // ── A proposal shows what would be, and applies nothing ──
    let proposed = ok(
        &http,
        &url,
        &atlas.key,
        "doc_apply",
        json!({
            "doc_ref": doc_ref,
            "mode": "propose",
            "ops": [{ "op": "set", "path": ["song", "tempo"], "value": 90 }],
        }),
    )
    .await;
    assert_eq!(proposed["applied"], json!(false));
    assert_eq!(proposed["would_be"]["song"]["tempo"], json!(90));
    assert_eq!(proposed["version"], json!(v2));

    // ── Undo reverts the apply, over a peer's later edit ──
    song.insert("key", "A").unwrap();
    peer.loro().commit();
    let mut seen = false;
    for _ in 0..400 {
        let r = ok(
            &http,
            &url,
            &atlas.key,
            "doc_read",
            json!({ "doc_ref": doc_ref }),
        )
        .await;
        if r["value"]["song"]["key"] == json!("A") {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(seen, "the server has the peer's edit");
    let undone = ok(
        &http,
        &url,
        &atlas.key,
        "doc_apply",
        json!({ "doc_ref": doc_ref, "ops": [{ "op": "undo", "apply": apply_id }] }),
    )
    .await;
    assert_eq!(undone["undid"], json!(apply_id), "{undone}");
    eventually("the peer sees the undo", || {
        field(&peer, "tempo") == json!(78) && field(&peer, "sections") == Value::Null
    })
    .await;
    assert_eq!(field(&peer, "key"), json!("A"), "the peer's own edit stays");
    let after = ok(
        &http,
        &url,
        &atlas.key,
        "doc_read",
        json!({ "doc_ref": doc_ref }),
    )
    .await;
    let applies = after["applies"].as_array().expect("the applies");
    assert_eq!(applies.len(), 2, "{after}");
    assert_eq!(applies[0]["apply"], json!(apply_id));
    assert_eq!(applies[0]["undone_by"], undone["apply"]);
    assert_eq!(applies[1]["undoes"], json!(apply_id));
    assert_eq!(applies[1]["created_by"], json!(atlas_ref));
    let again = refused(
        &http,
        &url,
        &atlas.key,
        "doc_apply",
        json!({ "doc_ref": doc_ref, "ops": [{ "op": "undo", "apply": apply_id }] }),
    )
    .await;
    assert!(
        again.contains("no applies left") || again.contains("already undone"),
        "{again}"
    );

    // ── The person's own apply is theirs ──
    let mine = ok(
        &http,
        &url,
        &token,
        "doc_apply",
        json!({ "doc_ref": doc_ref, "ops": [{ "op": "set", "path": ["song", "capo"], "value": 2 }] }),
    )
    .await;
    assert_eq!(
        mine["attribution"]["created_by"],
        json!(format!("user:{me}"))
    );
    eventually("the peer sees the person's edit", || {
        field(&peer, "capo") == json!(2)
    })
    .await;

    // ── An observer reads, and is refused the edit ──
    let watching = ok(
        &http,
        &url,
        &screen.key,
        "live_observe",
        json!({ "setlist": setlist }),
    )
    .await;
    assert_eq!(watching["songs"][0]["doc_ref"], json!(doc_ref));
    assert_eq!(
        watching["read_only"],
        json!(true),
        "it is told it only watches"
    );
    let seen = ok(
        &http,
        &url,
        &screen.key,
        "doc_read",
        json!({ "doc_ref": doc_ref }),
    )
    .await;
    assert_eq!(seen["value"]["song"]["capo"], json!(2));
    let no = refused(
        &http,
        &url,
        &screen.key,
        "doc_apply",
        json!({ "doc_ref": doc_ref, "ops": [{ "op": "set", "path": ["song", "tempo"], "value": 1 }] }),
    )
    .await;
    assert!(no.contains("only observes"), "{no}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        field(&peer, "tempo"),
        json!(78),
        "the observer changed nothing"
    );
}

// A ref that names no set, or a song the set doesn't have, says what
// there is to name.
#[tokio::test(flavor = "multi_thread")]
async fn a_bad_doc_ref_says_what_to_use() {
    let _env = crate::support::env_lock().await;
    let root = tempfile::tempdir().unwrap();
    let node = LocalNode::open(NodeOptions {
        owner_name: "Cody".into(),
        peers: false,
        ..NodeOptions::at(root.path())
    })
    .await
    .expect("the node opens");
    let base = node.serve_loopback().await.unwrap();
    let url = format!(
        "{}/org/{PERSONAL_SLUG}/mcp",
        base.replacen("ws://", "http://", 1)
    );
    let http = reqwest::Client::new();
    let home = node.state().home_identity.clone().unwrap();
    let owner = node.owner().clone();
    let me: Uuid = {
        let anon = node.caller(&owner.org, None).await.unwrap();
        AuthServiceClient::from_vox_lane(anon, None)
            .sign_in_email_password(SignInEmailPassword {
                email: owner.email.clone(),
                password: owner.password.clone(),
                ip_address: None,
                user_agent: Some("live-docs-e2e".into()),
            })
            .await
            .unwrap()
            .user
            .id
    };
    node.admit_account(me, &[PERSONAL_SLUG.to_owned()], "admin")
        .await
        .unwrap();
    let atlas = bots::create(
        &home.auth,
        &home.memberships,
        me,
        "Atlas",
        &OrgChoice::Some(vec![PERSONAL_SLUG.to_owned()]),
        Tools::All,
        bots::Kind::Team,
    )
    .await
    .unwrap();
    let org = node.state().org(PERSONAL_SLUG).unwrap();
    let set = org
        .collections
        .create(
            PERSONAL_SLUG.into(),
            "Set".into(),
            CollectionKind::new("songlist"),
        )
        .unwrap();
    org.collections
        .add_item(Placement {
            collection_id: set.id.clone(),
            node: NodeRef::song("washed"),
            after: None,
        })
        .unwrap();

    let e = refused(
        &http,
        &url,
        &atlas.key,
        "doc_read",
        json!({ "doc_ref": "washed" }),
    )
    .await;
    assert!(e.contains("live:<setlist>/<song>"), "{e}");
    let e = refused(
        &http,
        &url,
        &atlas.key,
        "doc_read",
        json!({ "doc_ref": format!("live:{}/nope", set.id) }),
    )
    .await;
    assert!(e.contains("washed"), "the set's songs are named: {e}");
    let e = refused(
        &http,
        &url,
        &atlas.key,
        "live_observe",
        json!({ "setlist": "no-such-set" }),
    )
    .await;
    assert!(e.contains("no setlist"), "{e}");
}

/// The same apply straight on a host, beside two replicas syncing over a
/// member's `DocSync` (the lanes the org mounts): what reaches a peer is
/// the served doc's own commit, whatever transport carries it.
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_on_the_host_reaches_every_replica() {
    use architect::{LayerRouter, LocalServer, Scope};
    use task_server::live::{LiveAccess, LiveHost, LiveOnly};
    use task_server::live_docs::{self, ApplyRequest, Attribution};

    let tmp = tempfile::tempdir().unwrap();
    let collections = collection::Store::open(tmp.path().join("collections.jsonl"));
    let set = collections
        .create(
            "live-test".into(),
            "Set".into(),
            CollectionKind::new("songlist"),
        )
        .unwrap();
    collections
        .add_item(Placement {
            collection_id: set.id.clone(),
            node: NodeRef::song("washed"),
            after: None,
        })
        .unwrap();
    let resources = resources::ResourcesBackend::new(tmp.path().join("resources"));
    let host = LiveHost::new("live-test".into(), collections, resources);
    let joined = host.join(&set.id, None).unwrap();
    let washed: Uuid = joined.songs[0].doc_id.parse().unwrap();

    let only = LiveOnly::new(host.clone(), LiveAccess::default());
    let local = LocalServer::serve(
        LayerRouter::new().with(
            crdt::sync::doc_sync_service_descriptor(),
            crdt::sync::DocSyncDispatcher::new(only),
        ),
        Scope::new(),
    );
    let mut replicas = Vec::new();
    for _ in 0..2 {
        let doc = CrdtDoc::ephemeral();
        let mut synced = SyncedDoc::new(washed, doc.clone());
        let client: DocSyncClient = local.establish().await.unwrap();
        tokio::spawn(async move {
            let _ = synced.run(&client).await;
        });
        replicas.push(doc);
    }
    replicas[0]
        .loro()
        .get_map("song")
        .insert("tempo", 78)
        .unwrap();
    replicas[0].loro().commit();
    eventually("the replicas meet", || {
        field(&replicas[1], "tempo") == json!(78)
    })
    .await;
    let served = host.registry().doc(washed).await.unwrap();
    for _ in 0..200 {
        if field(&served, "tempo") == json!(78) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let doc_ref = live_docs::doc_ref(&set.id, "washed");
    let applied = {
        let host = host.clone();
        tokio::task::spawn_blocking(move || {
            live_docs::apply(
                &host,
                &doc_ref,
                &ApplyRequest {
                    ops: &json!([{ "op": "set", "path": ["song", "tempo"], "value": 90 }]),
                    expected_version: None,
                    propose: false,
                    by: Attribution {
                        created_by: Some("bot:atlas".into()),
                        ..Attribution::default()
                    },
                },
            )
        })
        .await
        .unwrap()
        .expect("applied")
    };
    assert_eq!(applied["applied"], json!(true));
    for (i, r) in replicas.iter().enumerate() {
        eventually(&format!("replica {i} sees the apply"), || {
            field(r, "tempo") == json!(90)
        })
        .await;
    }
}
