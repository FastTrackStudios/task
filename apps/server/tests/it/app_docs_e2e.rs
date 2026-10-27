#![allow(clippy::large_futures)]
//! **An app's documents are on every device, and the server never reads
//! them.** `docs/spec/offline.md` `offline.app-docs.*`, driven through
//! the code a device runs (`crdt::docs::DocsDriver`, the driver Studio
//! starts at sign-in) against a booted server over a real WebSocket: the
//! org's `Docs` catalog (`task_server::app_docs`), its `changes` stream,
//! and the org's one `DocSync`, which routes catalogued ids to the
//! app-docs host.
//!
//! "Offline" is what it is on a device: no session running. Edits go into
//! the device's replica with nothing attached.

use std::sync::Arc;
use std::time::Duration;

use architect_auth::CreateEmailPasswordUser;
use crdt::CrdtDoc;
use crdt::docs::{
    CreateDoc, DocQuery, DocsClient, DocsDriver, DocsError, DocsLane, DocsStreamClient,
    InMemoryDocsIndex, SharedStorage,
};
use crdt::in_memory::InMemoryPersistence;
use crdt::sync::DocSyncClient;
use links_proto::{Confidence, LinksServiceClient, NodeKind, NodeRef, Reach, Relation, TypedLink};
use serde_json::{Value, json};
use task_server::{AppState, AuthState, capability::ServerKeypair, router};
use uuid::Uuid;

use crate::support;

const BODY: &str = "body";

async fn client<C: vox_core::FromVoxLane + vox::MaybeSend>(url: &str) -> C {
    vox::connect_lane(url)
        .establish()
        .await
        .unwrap_or_else(|e| panic!("connect: {e:?}"))
}

async fn lane(url: &str) -> DocsLane {
    DocsLane {
        docs: client(url).await,
        changes: client(url).await,
        sync: client(url).await,
    }
}

fn device() -> DocsDriver {
    DocsDriver::new(
        InMemoryDocsIndex::new(),
        SharedStorage(Arc::new(InMemoryPersistence::new())),
    )
}

/// The device online: one session, until the handle is aborted.
fn online(driver: &DocsDriver, lane: DocsLane) -> tokio::task::JoinHandle<()> {
    let driver = driver.clone();
    tokio::spawn(async move {
        let _ = driver.run(&lane).await;
    })
}

fn write(doc: &CrdtDoc, words: &str) {
    let text = doc.loro().get_text(BODY);
    text.insert(text.len_unicode(), words).unwrap();
    doc.loro().commit();
}

fn read(doc: &CrdtDoc) -> String {
    doc.loro().get_text(BODY).to_string()
}

async fn eventually(what: &str, mut cond: impl AsyncFnMut() -> bool) {
    for _ in 0..400 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn project(name: &str) -> CreateDoc {
    CreateDoc {
        doc_id: Uuid::new_v4(),
        kind: "session".into(),
        name: name.into(),
        parent: None,
        lineage: crdt::docs::Lineage::Strict,
    }
}

// t[verify offline.app-docs.create] — a doc made on a device with no
// connection registers when it returns, and another device adopts it
// t[verify offline.app-docs.replica] — two devices edited apart converge,
// and the server keeps the catalog and the bytes across a restart
// t[verify offline.app-docs.tombstone] — a delete reaches a device that
// was away
#[tokio::test(flavor = "multi_thread")]
async fn app_docs_made_and_edited_apart_converge_and_survive_a_restart() {
    let (url, tmp) = support::boot_ws().await.expect("boot");
    let (laptop, phone) = (device(), device());

    // Made on the laptop with nothing attached, then a branch of it.
    let req = project("CAA set");
    let id = req.doc_id;
    let on_laptop = laptop.create(req).await.unwrap();
    write(&on_laptop, "intro ");
    let branch = CreateDoc {
        parent: Some(id),
        ..project("CAA set (acoustic)")
    };
    laptop.create(branch.clone()).await.unwrap();

    let run_laptop = online(&laptop, lane(&url).await);
    let run_phone = online(&phone, lane(&url).await);
    eventually("the phone adopts the laptop's project", async || {
        phone.open(id).await.is_ok_and(|d| read(&d) == "intro ")
    })
    .await;
    let docs: DocsClient = client(&url).await;
    let held = docs
        .get(branch.doc_id)
        .await
        .expect("the branch registered");
    assert_eq!(held.parent, Some(id), "lineage is carried, not read");
    assert_eq!(held.kind, "session", "the kind is the app's word");

    // Both offline; each edits.
    run_laptop.abort();
    run_phone.abort();
    let _ = (run_laptop.await, run_phone.await);
    let on_phone = phone.open(id).await.unwrap();
    write(&on_laptop, "laptop ");
    write(&on_phone, "phone ");

    // Deleted elsewhere while both are away.
    docs.delete(branch.doc_id).await.expect("delete");

    let run_laptop = online(&laptop, lane(&url).await);
    let run_phone = online(&phone, lane(&url).await);
    eventually("the two copies converge", async || {
        let (a, b) = (read(&on_laptop), read(&on_phone));
        a == b && a.contains("laptop ") && a.contains("phone ")
    })
    .await;
    eventually("both devices learn of the delete", async || {
        matches!(laptop.open(branch.doc_id).await, Err(DocsError::Deleted))
            && matches!(phone.open(branch.doc_id).await, Err(DocsError::Deleted))
    })
    .await;
    let merged = read(&on_laptop);
    run_laptop.abort();
    run_phone.abort();

    // A second server over the same data root: the catalog and the bytes
    // were in `app_docs.sqlite`, so a new device gets both.
    let (url2, ()) = support::boot_ws_over(&tmp).await.expect("reboot");
    let fresh = device();
    let run_fresh = online(&fresh, lane(&url2).await);
    eventually("a new device gets the merged project", async || {
        fresh.open(id).await.is_ok_and(|d| read(&d) == merged)
    })
    .await;
    let listed: DocsClient = client(&url2).await;
    let page = listed
        .list(DocQuery {
            include_deleted: true,
            ..DocQuery::default()
        })
        .await
        .unwrap();
    assert!(
        page.docs
            .iter()
            .any(|d| d.doc_id == branch.doc_id && d.deleted),
        "the tombstone survives the restart"
    );
    run_fresh.abort();
}

// t[verify offline.app-docs.merge] — a doc every device makes offline
// under one derived id, with Merge lineage, converges with both
// devices' edits instead of being refused
/// A device's lane through the dial Studio uses (`task_dial`), whose
/// limits are the link's, not the WebSocket library's defaults.
async fn dialed_lane(url: &str) -> DocsLane {
    DocsLane {
        docs: task_dial::establish_at(url, None).await.expect("docs"),
        changes: task_dial::establish_at(url, None).await.expect("changes"),
        sync: task_dial::establish_at(url, None).await.expect("sync"),
    }
}

// A song doc's first sync is one vox message, larger than the 16 MiB a
// WebSocket frame was allowed: it reaches the org and the next device.
// Ignored by default (20 MiB through a debug build is slow): run it with
// `cargo nextest run -p task-server --test it --run-ignored only -E
// 'test(over_16_mib)'` when the link's limits change.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: 20 MiB through the link; run with --run-ignored when the link's limits change"]
async fn a_doc_whose_first_sync_is_over_16_mib_reaches_the_org_and_the_next_device() {
    let (url, _tmp) = support::boot_ws().await.expect("boot");
    let (desk, ipad) = (device(), device());
    let req = project("Unbreakable");
    let id = req.doc_id;
    let on_desk = desk.create(req).await.unwrap();
    // 20 MiB that does not compress: a real song's items do not either.
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let words: String = (0..20 << 20)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            char::from(b'a' + (seed % 26) as u8)
        })
        .collect();
    write(&on_desk, &words);

    let _desk = online(&desk, dialed_lane(&url).await);
    let _ipad = online(&ipad, dialed_lane(&url).await);
    // Tens of MB through a debug build: longer than `eventually` allows.
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !ipad
        .open(id)
        .await
        .is_ok_and(|d| read(&d).len() == words.len())
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the iPad never held the whole song"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_merge_doc_made_on_two_devices_offline_converges_through_the_org() {
    let (url, _tmp) = support::boot_ws().await.expect("boot");
    let (laptop, phone) = (device(), device());
    let id = Uuid::new_v4(); // stands for the rig's derived (v5) id
    let rig = || CreateDoc {
        doc_id: id,
        kind: "rig".into(),
        name: "Bass rig".into(),
        parent: None,
        lineage: crdt::docs::Lineage::Merge,
    };
    let put = |doc: &CrdtDoc, key: &str, value: &str| {
        doc.loro().get_map("library").insert(key, value).unwrap();
        doc.loro().commit();
    };
    let get = |doc: &CrdtDoc, key: &str| {
        doc.loro()
            .get_map("library")
            .get(key)
            .and_then(|v| v.into_value().ok())
            .and_then(|v| v.into_string().ok())
            .map(|s| s.to_string())
    };
    let on_laptop = laptop.create(rig()).await.unwrap();
    put(&on_laptop, "amp", "laptop");
    let on_phone = phone.create(rig()).await.unwrap();
    put(&on_phone, "cab", "phone");
    let mut refused = phone.sync_events();

    let run_laptop = online(&laptop, lane(&url).await);
    let run_phone = online(&phone, lane(&url).await);
    eventually("both copies hold both entries", async || {
        [&on_laptop, &on_phone].iter().all(|d| {
            get(d, "amp").as_deref() == Some("laptop") && get(d, "cab").as_deref() == Some("phone")
        })
    })
    .await;
    while let Ok(ev) = refused.try_recv() {
        assert!(
            !matches!(ev, crdt::docs::DocSyncEvent::Refused { .. }),
            "{ev:?}"
        );
    }
    let docs: DocsClient = client(&url).await;
    assert_eq!(
        docs.get(id).await.unwrap().lineage,
        crdt::docs::Lineage::Merge,
        "the org keeps the policy"
    );
    run_laptop.abort();
    run_phone.abort();
}

// t[verify offline.app-docs.face] — a face written on a device reads on
// the org, listed by kind and singly; a doc without one is titled by its
// catalog name; a deleted doc is refused
#[tokio::test(flavor = "multi_thread")]
async fn the_org_lists_its_docs_by_the_face_each_keeps() {
    use app_docs_proto::{
        AppDocFacesClient, DocFace, FaceLink, FaceProp, FacePropType, FaceValue, FacesError,
        write_face,
    };

    let (url, _tmp) = support::boot_ws().await.expect("boot");
    let laptop = device();
    let list_req = CreateDoc {
        kind: "app.list".into(),
        ..project("untitled list")
    };
    let id = list_req.doc_id;
    let list = laptop.create(list_req).await.unwrap();
    let bare_req = CreateDoc {
        kind: "app.item".into(),
        ..project("Bare item")
    };
    let bare = bare_req.doc_id;
    laptop.create(bare_req).await.unwrap();
    let face = DocFace {
        title: "Sunday 10/26".into(),
        subtitle: Some("The Barn".into()),
        colour: Some("#22c55e".into()),
        props: vec![
            FaceProp {
                key: "entries".into(),
                label: "Entries".into(),
                ty: FacePropType::Number,
                value: FaceValue::Integer(12),
            },
            FaceProp {
                key: "tags".into(),
                label: "Tags".into(),
                ty: FacePropType::Tags,
                value: FaceValue::Tags(vec!["live".into()]),
            },
        ],
        links: vec![FaceLink {
            rel: "holds".into(),
            label: "Bare item".into(),
            doc: bare,
        }],
    };
    write_face(list.loro(), &face).unwrap();
    list.loro().commit();
    let run = online(&laptop, lane(&url).await);

    let faces: AppDocFacesClient = client(&url).await;
    eventually("the org reads the face", async || {
        faces
            .face(id)
            .await
            .is_ok_and(|c| c.face.as_ref() == Some(&face))
    })
    .await;
    let card = faces.face(id).await.unwrap();
    assert_eq!(card.title, "Sunday 10/26", "the face titles it");
    assert_eq!(card.name, "untitled list", "the catalog's name is kept");
    assert_eq!(card.kind, "app.list");

    // Listed, narrowed by kind; a faceless doc is titled by its name.
    let lists = faces.list_faces(Some("app.list".into()), 0).await.unwrap();
    assert_eq!(lists.len(), 1, "{lists:?}");
    assert_eq!(lists[0].face.as_ref(), Some(&face));
    let all = faces.list_faces(None, 0).await.unwrap();
    let item = all.iter().find(|c| c.doc_id == bare).expect("listed");
    assert_eq!(
        (item.face.as_ref(), item.title.as_str()),
        (None, "Bare item")
    );
    assert_eq!(faces.list_faces(None, 1).await.unwrap().len(), 1, "limit");

    // A project holds the list (its `project:` → `doc:` link); its
    // faces carry the comments on each doc.
    let projects: project::ProjectServiceClient = client(&url).await;
    let recital = projects
        .create(project::ProjectInfo {
            title: "Senior Recital".into(),
            project_type: "general".into(),
            ..Default::default()
        })
        .await
        .expect("create project");
    let tier = task_server::app_docs::project_tier_path(&recital.path).to_owned();
    let links: LinksServiceClient = client(&url).await;
    links
        .create(TypedLink::new(
            NodeRef::new(NodeKind::Project, tier),
            NodeRef::new(NodeKind::AppDoc, id.to_string()),
            Relation::Down,
            Confidence::Certain,
        ))
        .await
        .expect("attach");
    let threads: threads::ThreadsServiceClient = client(&url).await;
    let thread = threads
        .create_thread(threads::CreateThreadRequest {
            org_id: Uuid::nil(),
            entity_type: "doc".into(),
            entity_id: id,
            anchor_path: None,
            title: "Comments".into(),
            kind: String::new(),
            created_by: Uuid::nil(),
            source_kind: String::new(),
            source_ref: None,
            source_url: None,
        })
        .await
        .expect("thread");
    for body in ["Open with the Bach?", "Yes."] {
        threads
            .post_message(threads::PostMessageRequest {
                thread_id: thread.id,
                org_id: Uuid::nil(),
                author_id: None,
                author_label: "Sam".into(),
                body: body.into(),
                reply_to: None,
                source_kind: String::new(),
                external_id: None,
                original_text: None,
                source_url: None,
                posted_at: None,
            })
            .await
            .expect("post");
    }
    let held = faces.project_faces(recital.id).await.unwrap();
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].doc_id, id);
    assert_eq!(held[0].face.as_ref(), Some(&face));
    assert_eq!(held[0].comments, 2, "the doc's comments are counted");
    assert!(faces.project_faces(Uuid::new_v4()).await.is_err());

    // Deleted: refused by name; an id the catalog lacks, likewise.
    let docs: DocsClient = client(&url).await;
    docs.delete(bare).await.unwrap();
    let refused = |r: Result<_, vox::VoxError<FacesError>>| match r {
        Err(vox::VoxError::User(e)) => Some(*e),
        _ => None,
    };
    assert!(matches!(
        refused(faces.face(bare).await),
        Some(FacesError::Deleted(_))
    ));
    assert!(matches!(
        refused(faces.face(Uuid::new_v4()).await),
        Some(FacesError::NotFound(_))
    ));
    assert!(
        faces
            .list_faces(None, 0)
            .await
            .unwrap()
            .iter()
            .all(|c| c.doc_id != bare),
        "a tombstone is not listed"
    );
    run.abort();
}

const MCP_TOKEN: &str = "app-docs-mcp-secret";

/// One MCP `tools/call`: the whole JSON-RPC answer.
async fn mcp(
    http: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    name: &str,
    args: Value,
) -> Value {
    let mut req = http.post(url).json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": args },
    }));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    req.send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("json-rpc body")
}

/// [`mcp`] as the org's agent: whether the tool failed, and its payload.
async fn tool(http: &reqwest::Client, url: &str, name: &str, args: Value) -> (bool, Value) {
    let res = mcp(http, url, Some(MCP_TOKEN), name, args).await;
    let result = &res["result"];
    assert!(!result.is_null(), "{name}: protocol error {res}");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    let payload = serde_json::from_str(text).unwrap_or_else(|_| json!(text));
    (result["isError"].as_bool().unwrap_or(false), payload)
}

// A project names the app docs that are its own, with Task treating the
// doc's kind as the app's word: list the catalog, attach (once), list
// the project's docs from the catalog, keep a deleted one listed as
// gone, detach, and refuse an id the catalog does not hold. Driven
// through the MCP tools, beside the vox `Docs` lane an app registers on.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_lists_the_app_docs_attached_to_it() {
    let (ws, _tmp) = support::boot_ws_env(&[("TASK_MCP_TOKEN", MCP_TOKEN)])
        .await
        .expect("boot");
    let origin = ws.trim_end_matches("/vox");
    let vox = format!("{origin}/org/{}/vox", support::ORG);
    let url = format!(
        "{}/org/{}/mcp",
        origin.replacen("ws://", "http://", 1),
        support::ORG
    );
    let http = reqwest::Client::new();

    // An app registers two docs of its own kinds.
    let docs: DocsClient = client(&vox).await;
    let session = docs.create(project("Unbreakable")).await.expect("register");
    let setlist = docs
        .create(CreateDoc {
            kind: "setlist".into(),
            ..project("Sunday")
        })
        .await
        .expect("register");
    let id = session.doc_id.to_string();

    // The catalog lists them with their kinds passed through.
    let (failed, listed) = tool(&http, &url, "list_app_docs", json!({ "kind": "session" })).await;
    assert!(!failed, "{listed}");
    let rows = listed["docs"].as_array().expect("docs");
    assert!(
        rows.iter()
            .any(|r| r["id"] == id && r["kind"] == "session" && r["name"] == "Unbreakable"),
        "{listed}"
    );
    assert!(
        rows.iter().all(|r| r["id"] != setlist.doc_id.to_string()),
        "`kind` narrows: {listed}"
    );

    // Attaching is idempotent, and an id the catalog lacks is refused.
    let args = json!({ "project": "launch-ep", "doc_id": id });
    let (failed, first) = tool(&http, &url, "attach_doc", args.clone()).await;
    assert!(!failed, "{first}");
    let (_, again) = tool(&http, &url, "attach_doc", args.clone()).await;
    assert_eq!(first["link_id"], again["link_id"], "one link, not two");
    let (failed, refused) = tool(
        &http,
        &url,
        "attach_doc",
        json!({ "project": "launch-ep", "doc_id": Uuid::new_v4().to_string() }),
    )
    .await;
    assert!(
        failed && refused.to_string().contains("catalog"),
        "{refused}"
    );

    // The project lists it, enriched from the catalog.
    let (_, held) = tool(
        &http,
        &url,
        "project_docs",
        json!({ "project": "launch-ep" }),
    )
    .await;
    assert_eq!(held["count"], 1, "{held}");
    assert_eq!(held["docs"][0]["id"], id);
    assert_eq!(held["docs"][0]["kind"], "session");
    assert_eq!(held["docs"][0]["name"], "Unbreakable");
    assert!(held["docs"][0]["gone"].is_null(), "{held}");

    // A local `doc:` reference resolves against the catalog.
    let links: LinksServiceClient = client(&vox).await;
    let missing = NodeRef::new(NodeKind::AppDoc, Uuid::new_v4().to_string());
    let answers = links
        .resolve_nodes(vec![NodeRef::new(NodeKind::AppDoc, id.clone()), missing])
        .await
        .expect("resolve_nodes");
    assert_eq!(answers[0].reach, Reach::Local);
    assert_eq!(answers[1].reach, Reach::NotFound);

    // Deleted in the app: still listed, as gone, until detached.
    docs.delete(session.doc_id).await.expect("delete");
    let (_, held) = tool(
        &http,
        &url,
        "project_docs",
        json!({ "project": "launch-ep" }),
    )
    .await;
    assert_eq!(held["docs"][0]["gone"], "deleted", "{held}");
    let (failed, out) = tool(&http, &url, "detach_doc", args.clone()).await;
    assert!(!failed && out["detached"] == 1, "{out}");
    let (_, held) = tool(
        &http,
        &url,
        "project_docs",
        json!({ "project": "launch-ep" }),
    )
    .await;
    assert_eq!(held["count"], 0, "{held}");
    let (failed, _) = tool(&http, &url, "detach_doc", args).await;
    assert!(failed, "detaching what is not attached says so");

    // Not a member: refused before any tool runs.
    for token in [None, Some("not-the-token")] {
        let res = mcp(
            &http,
            &url,
            token,
            "project_docs",
            json!({ "project": "launch-ep" }),
        )
        .await;
        assert!(!res["error"].is_null(), "{token:?}: {res}");
    }
}

/// An ENFORCING server with one org, for the access checks.
async fn boot_enforcing() -> eyre::Result<(String, AuthState, tempfile::TempDir)> {
    const ORG: &str = "app-docs-access";
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = support::env_lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org(ORG, "App Docs Access", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral()).await?;
    drop(guard);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    Ok((format!("ws://127.0.0.1:{port}/org/{ORG}/vox"), auth, tmp))
}

/// Dial presenting `token` the way a browser does.
async fn dial<C: vox_core::FromVoxLane + vox::MaybeSend>(
    url: &str,
    token: Option<&str>,
) -> eyre::Result<C> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let mut request = url.into_client_request()?;
    let offered = token.map_or_else(
        || "vox.v1".to_owned(),
        |t| format!("vox.v1, vox.bearer.{t}"),
    );
    request
        .headers_mut()
        .insert("sec-websocket-protocol", offered.parse()?);
    let (stream, _response) = tokio_tungstenite::connect_async(request).await?;
    vox_core::initiator_on(vox_websocket::WsLink::new(stream))
        .establish::<C>()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))
}

// t[verify offline.app-docs.access] — the catalog and the content answer
// the org's members only
#[tokio::test(flavor = "multi_thread")]
async fn only_members_reach_the_catalog_and_the_content() -> eyre::Result<()> {
    let (url, auth, _tmp) = boot_enforcing().await?;
    let token = auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "player@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Player".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?
        .token;

    // A member registers a doc; the row names who did.
    let member: DocsClient = dial(&url, Some(&token)).await?;
    let req = project("Rig");
    let made = member
        .create(req.clone())
        .await
        .map_err(|e| eyre::eyre!("a member registers: {e:?}"))?;
    assert!(!made.created_by.is_empty(), "the host stamps the creator");

    // Anonymous: refused on the catalog, the stream and the content.
    let anonymous: DocsClient = dial(&url, None).await?;
    let refused = format!("{:?}", anonymous.list(DocQuery::default()).await);
    assert!(refused.contains("permission denied"), "list: {refused}");
    let refused = format!("{:?}", anonymous.create(project("Not mine")).await);
    assert!(refused.contains("permission denied"), "create: {refused}");
    let faces: app_docs_proto::AppDocFacesClient = dial(&url, None).await?;
    let refused = format!("{:?}", faces.list_faces(None, 0).await);
    assert!(
        refused.contains("permission denied"),
        "list_faces: {refused}"
    );
    let refused = format!("{:?}", faces.face(req.doc_id).await);
    assert!(refused.contains("permission denied"), "face: {refused}");
    let stream: DocsStreamClient = dial(&url, None).await?;
    let (tx, _rx) = vox::channel();
    let refused = format!(
        "{:?}",
        tokio::time::timeout(Duration::from_secs(5), stream.changes(tx)).await
    );
    assert!(refused.contains("permission denied"), "changes: {refused}");
    let sync: DocSyncClient = dial(&url, None).await?;
    let (_up_tx, up_rx) = vox::channel::<Vec<u8>>();
    let (down_tx, _down_rx) = vox::channel();
    let refused = format!(
        "{:?}",
        tokio::time::timeout(
            Duration::from_secs(5),
            sync.sync(req.doc_id, Vec::new(), up_rx, down_tx)
        )
        .await
    );
    assert!(refused.contains("permission denied"), "sync: {refused}");

    // Nor may it attach the doc to a project, or read what is attached.
    let project_ref = NodeRef::new(NodeKind::Project, "rig-project");
    let doc_ref = NodeRef::new(NodeKind::AppDoc, req.doc_id.to_string());
    let links: LinksServiceClient = dial(&url, None).await?;
    let refused = format!(
        "{:?}",
        links
            .create(TypedLink::new(
                project_ref.clone(),
                doc_ref.clone(),
                Relation::Down,
                Confidence::Certain,
            ))
            .await
    );
    assert!(refused.contains("permission denied"), "attach: {refused}");
    let refused = format!("{:?}", links.links_for(project_ref.clone()).await);
    assert!(
        refused.contains("permission denied"),
        "project docs: {refused}"
    );
    // The member may.
    let member_links: LinksServiceClient = dial(&url, Some(&token)).await?;
    member_links
        .create(TypedLink::new(
            project_ref,
            doc_ref,
            Relation::Down,
            Confidence::Certain,
        ))
        .await
        .map_err(|e| eyre::eyre!("a member attaches: {e:?}"))?;
    Ok(())
}
