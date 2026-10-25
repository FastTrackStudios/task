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
    Ok(())
}
