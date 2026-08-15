#![allow(clippy::large_futures)]
//! Vault-file ⇄ CRDT reconciliation, end to end over the same mounts
//! `org_layer_router` uses: the `VaultSync` service (file wire), the
//! `DocSync` service (vault-collab `DocRegistry`), and the write-behind
//! / inbound loops between them.
//!
//! Scenario (the survey's "hard slice" acceptance shape): two
//! [`crdt::sync::SyncedDoc`] clients collaborate on one vault file
//! while a third, **non-CRDT** writer mutates the same file through
//! plain `put_file` — everyone (client A, client B, the file on disk)
//! must converge, and the write-behind's own broadcasts must not
//! re-apply (echo guard).
//!
//! The live `ws://` variant of this scenario is
//! [`live_smoke_two_clients_plus_put_file`] (`#[ignore]` — needs a
//! running `task-server`; see the doc comment there).

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use crdt::CrdtDoc;
use crdt::sync::{DocSyncClient, SyncedDoc};
use uuid::Uuid;
use vault_proto::{COLLAB_TEXT_CONTAINER, IfMatch, VaultSyncClient};

const DEBOUNCE: Duration = Duration::from_millis(100);

async fn eventually(what: &str, mut cond: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// A collaborating replica: ephemeral local doc + a running sync
/// session against the served registry.
async fn join(local: &LocalServer, doc_id: Uuid) -> CrdtDoc {
    let doc = CrdtDoc::ephemeral();
    let mut synced = SyncedDoc::new(doc_id, doc.clone());
    let client: DocSyncClient = local.establish().await.expect("establish DocSyncClient");
    tokio::spawn(async move {
        let _ = synced.run(&client).await;
    });
    doc
}

fn text_of(doc: &CrdtDoc) -> String {
    doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn two_synced_clients_plus_put_file_writer_converge() {
    let tmp = tempfile::tempdir().unwrap();
    let vault_dir = tmp.path().join("vault");
    let backend = vault::Backend::single("default", vault_dir.clone()).unwrap();
    let collab = vault_collab::VaultCollab::with_debounce(
        backend.clone(),
        tmp.path().join("crdt"),
        DEBOUNCE,
    );
    collab.watch_vault("default");

    // The same two mounts `org_layer_router` adds for this feature.
    let router = LayerRouter::new()
        .with(
            vault_proto::descriptor(),
            vault_proto::serve(backend.clone()),
        )
        .with(
            crdt::sync::doc_sync_service_descriptor(),
            crdt::sync::DocSyncDispatcher::new(collab.registry().clone()),
        );
    let scope = Scope::new();
    let local = LocalServer::serve(router, scope.clone());

    // Non-CRDT writer: creates the file over the plain wire, then
    // registers it for collaboration.
    let writer: VaultSyncClient = local.establish().await.expect("establish writer");
    writer
        .put_file(
            "default".into(),
            "notes/shared.md".into(),
            b"base\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let ack = writer
        .open_collab("default".into(), "notes/shared.md".into())
        .await
        .unwrap();
    assert_eq!(
        ack.doc_id,
        vault_proto::collab_doc_id("default", "notes/shared.md")
    );

    // Two synced replicas join and receive the seeded content.
    let doc_a = join(&local, ack.doc_id).await;
    let doc_b = join(&local, ack.doc_id).await;
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("A seeded", async || text_of(&a) == "base\n").await;
    eventually("B seeded", async || text_of(&b) == "base\n").await;

    // A types; B and the file on disk must observe it (relay + flush).
    doc_a
        .loro()
        .get_text(COLLAB_TEXT_CONTAINER)
        .insert(0, "alpha ")
        .unwrap();
    doc_a.loro().commit();
    let b = doc_b.clone();
    eventually("B sees A's edit", async || text_of(&b) == "alpha base\n").await;
    let disk = vault_dir.join("notes/shared.md");
    let d = disk.clone();
    eventually("write-behind flushes A's edit", async || {
        std::fs::read_to_string(&d).ok().as_deref() == Some("alpha base\n")
    })
    .await;

    // B types too — both replicas converge and the merged text lands
    // on disk with a coherent sha for sha-clients.
    doc_b
        .loro()
        .get_text(COLLAB_TEXT_CONTAINER)
        .insert(11, "beta\n")
        .unwrap();
    doc_b.loro().commit();
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("replicas converge on A+B edits", async || {
        let (ta, tb) = (text_of(&a), text_of(&b));
        ta == tb && ta.contains("alpha") && ta.contains("beta")
    })
    .await;
    let d = disk.clone();
    let a = doc_a.clone();
    eventually("merged text flushed", async || {
        std::fs::read_to_string(&d).ok() == Some(text_of(&a))
    })
    .await;

    // A raw put_file interleaves: a non-CRDT client overwrites the
    // file (its view = the last flush) with an appended line. The
    // inbound listener must merge it into the open doc and the
    // replicas must converge on it.
    let current = String::from_utf8(
        writer
            .get_file("default".into(), "notes/shared.md".into())
            .await
            .unwrap()
            .0,
    )
    .unwrap();
    writer
        .put_file(
            "default".into(),
            "notes/shared.md".into(),
            format!("{current}from-file\n").into_bytes(),
            IfMatch::Force,
        )
        .await
        .unwrap();
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("replicas pick up the raw put_file", async || {
        let (ta, tb) = (text_of(&a), text_of(&b));
        ta == tb && ta.contains("from-file") && ta.contains("alpha") && ta.contains("beta")
    })
    .await;

    // Quiesce, then assert the echo guard held: nothing re-applied,
    // nothing duplicated; disk == replicas, single copy of each edit.
    tokio::time::sleep(DEBOUNCE * 5).await;
    let final_a = text_of(&doc_a);
    assert_eq!(final_a, text_of(&doc_b), "replicas diverged");
    assert_eq!(
        std::fs::read_to_string(&disk).unwrap(),
        final_a,
        "disk diverged from replicas"
    );
    assert_eq!(
        final_a.matches("from-file").count(),
        1,
        "inbound applied twice"
    );
    assert_eq!(
        final_a.matches("alpha").count(),
        1,
        "echo re-applied A's edit"
    );

    scope.close().await;
}

/// Live smoke over a real WebSocket — the same scenario as above
/// against a *running* `task-server`. Ignored by default; run with:
///
/// ```text
/// TASK_COLLAB_SMOKE_URL=ws://127.0.0.1:18081/org/<slug>/vox \
///   cargo test -p task-server --test vault_collab_e2e -- --ignored live_smoke
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running task-server (set TASK_COLLAB_SMOKE_URL)"]
async fn live_smoke_two_clients_plus_put_file() {
    let url = std::env::var("TASK_COLLAB_SMOKE_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:18081/org/fasttrackstudios/vox".to_string());
    let path = format!("collab-smoke/{}.md", uuid::Uuid::new_v4());

    let writer: VaultSyncClient = vox::connect_lane(&url)
        .establish()
        .await
        .expect("ws connect");
    writer
        .put_file(
            "default".into(),
            path.clone(),
            b"smoke base\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let ack = writer
        .open_collab("default".into(), path.clone())
        .await
        .unwrap();
    eprintln!("live smoke: doc_id {} for {path}", ack.doc_id);

    // Two ws replicas.
    let mut docs = Vec::new();
    for label in ["A", "B"] {
        let doc = CrdtDoc::ephemeral();
        let mut synced = SyncedDoc::new(ack.doc_id, doc.clone());
        let client: DocSyncClient = vox::connect_lane(&url)
            .establish()
            .await
            .unwrap_or_else(|e| panic!("ws connect {label}: {e:?}"));
        tokio::spawn(async move {
            let _ = synced.run(&client).await;
        });
        docs.push(doc);
    }
    let (doc_a, doc_b) = (docs[0].clone(), docs[1].clone());
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("A seeded", async || text_of(&a) == "smoke base\n").await;
    eventually("B seeded", async || text_of(&b) == "smoke base\n").await;

    // A types, B observes; write-behind commits to the real vault.
    doc_a
        .loro()
        .get_text(COLLAB_TEXT_CONTAINER)
        .insert(0, "hello ")
        .unwrap();
    doc_a.loro().commit();
    let b = doc_b.clone();
    eventually("B sees A's edit", async || {
        text_of(&b) == "hello smoke base\n"
    })
    .await;
    let w = writer.clone();
    let p = path.clone();
    eventually("server file matches (write-behind)", async || {
        w.get_file("default".into(), p.clone())
            .await
            .map(|bytes| bytes.0 == b"hello smoke base\n")
            .unwrap_or(false)
    })
    .await;

    // Raw put_file interleaves; replicas converge on the merge.
    writer
        .put_file(
            "default".into(),
            path.clone(),
            b"hello smoke base\nfrom-file\n".to_vec(),
            IfMatch::Force,
        )
        .await
        .unwrap();
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("replicas pick up raw put_file", async || {
        let (ta, tb) = (text_of(&a), text_of(&b));
        ta == tb && ta.contains("from-file") && ta.contains("hello")
    })
    .await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let final_a = text_of(&doc_a);
    assert_eq!(final_a, text_of(&doc_b));
    let server_copy = writer
        .get_file("default".into(), path.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(String::from_utf8(server_copy).unwrap(), final_a);
    assert_eq!(final_a.matches("from-file").count(), 1);
    // When the server runs on this machine, also assert the raw file
    // on disk (TASK_COLLAB_SMOKE_DISK = the served vault dir).
    if let Ok(vault_dir) = std::env::var("TASK_COLLAB_SMOKE_DISK") {
        let disk = std::fs::read_to_string(std::path::Path::new(&vault_dir).join(&path)).unwrap();
        assert_eq!(disk, final_a, "file on disk diverged from replicas");
        eprintln!("live smoke: disk file matches replicas");
    }
    eprintln!("live smoke: converged text:\n{final_a}");

    // Tidy the smoke file.
    let _ = writer
        .delete_file("default".into(), path, IfMatch::Force)
        .await;
}
