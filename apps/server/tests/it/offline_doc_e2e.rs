#![allow(clippy::large_futures)]
//! **Documents edited apart converge, and the server writes the files.**
//! `docs/spec/offline.md` — `offline.doc.replica`, `offline.doc.create`,
//! `offline.doc.materialise`, `offline.doc.access` — driven through the
//! code a device runs (`vault_doc::Replica`, the same sessions the shell's
//! driver opens) against a booted server over a real WebSocket: the
//! `DocSync` lane under its guard (`doc_guard`), the vault-collab registry,
//! its mirror write-behind and the vault watcher.
//!
//! "Offline" here is exactly what it is on a device: no session running.
//! Each replica persists in its own store (a directory, standing in for
//! the device database), edits go into it with nothing attached, and a
//! session is opened when the test says the connection is back.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crdt::loro::LoroValue;
use crdt::sync::DocSyncClient;
use uuid::Uuid;
use vault_doc::{Replica, SessionEnd, SessionEvent, SharedStore};
use vault_proto::{IfMatch, VaultSyncClient};

use crate::support;

const VAULT: &str = "default";

async fn lane<C: vox_core::FromVoxLane + vox::MaybeSend>(url: &str) -> C {
    vox::connect_lane(url)
        .establish()
        .await
        .unwrap_or_else(|e| panic!("connect: {e:?}"))
}

fn store(dir: &std::path::Path) -> SharedStore {
    SharedStore(Arc::new(crdt::FilePersistence::new(dir)))
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

/// The connection comes back: register the file (`open_collab`, the
/// server's admission and write check) and run one session until the
/// server holds everything this replica has — then drop it (offline
/// again). Returns how the session went.
async fn sync_once(url: &str, r: &mut Replica, path: &str) -> SessionEnd {
    let vault: VaultSyncClient = lane(url).await;
    if let Err(e) = vault.open_collab(VAULT.into(), path.into()).await {
        return SessionEnd::Refused(format!("{e:?}"));
    }
    sync_registered(url, r).await
}

async fn sync_registered(url: &str, r: &mut Replica) -> SessionEnd {
    let client: DocSyncClient = lane(url).await;
    let attached = Rc::new(Cell::new(false));
    let unsynced = Rc::new(Cell::new(r.unsynced()));
    let (a, u) = (attached.clone(), unsynced.clone());
    let session = r.run(&client, move |ev| match ev {
        SessionEvent::Attached => a.set(true),
        SessionEvent::Acked { unsynced } => u.set(unsynced),
    });
    let settled = async {
        loop {
            tokio::time::sleep(Duration::from_millis(25)).await;
            if attached.get() && !unsynced.get() {
                // Let a last relay land, then disconnect.
                tokio::time::sleep(Duration::from_millis(100)).await;
                return;
            }
        }
    };
    tokio::select! {
        end = session => end,
        () = settled => SessionEnd::Dropped,
        // Generous: under a loaded machine a sync can take a while to settle.
        () = tokio::time::sleep(Duration::from_secs(45)) => panic!("session never settled"),
    }
}

/// Wait until the store has at least `n` updates for `id` (the
/// persistence writer is asynchronous), so a reopen sees them.
async fn persisted(st: &SharedStore, id: Uuid, n: usize) {
    eventually("the store to hold the edit", async || {
        crdt::Persistence::load_updates(st, id)
            .await
            .map(|u| u.len() >= n)
            .unwrap_or(false)
    })
    .await;
}

fn render(r: &Replica) -> String {
    vault_doc::render(r.doc().loro(), None)
}

fn edit(r: &Replica, base: &str, new: &str) {
    vault_doc::merge_text(r.doc().loro(), base, new);
    vault_doc::commit_as(r.doc().loro(), "test edit");
}

// t[verify offline.doc.replica] — two replicas edited while disconnected
// (one of them restarted in between) converge on reconnect with both
// people's words and both property changes; the server writes the file
// t[verify offline.doc.materialise] — the mirror is rendered from the
// merged document, frontmatter from props
#[tokio::test(flavor = "multi_thread")]
async fn two_replicas_edited_apart_converge_and_the_server_writes_the_file() {
    let (url, tmp) = support::boot_ws().await.expect("boot");
    let vault: VaultSyncClient = lane(&url).await;
    let path = "Offline/Setlist notes.md";
    let original = "---\ntitle: Setlist notes\nstatus: draft\n---\nOpen with the ballad.\n";
    vault
        .put_file(
            VAULT.into(),
            path.into(),
            original.as_bytes().to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let doc_id = vault_proto::collab_doc_id(VAULT, path);

    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (st_a, st_b) = (store(dir_a.path()), store(dir_b.path()));
    let mut a = Replica::open(doc_id, st_a.clone(), None).await.unwrap();
    let mut b = Replica::open(doc_id, st_b.clone(), None).await.unwrap();
    assert_eq!(sync_once(&url, &mut a, path).await, SessionEnd::Dropped);
    assert_eq!(sync_once(&url, &mut b, path).await, SessionEnd::Dropped);
    assert_eq!(
        render(&a),
        original.replace(
            "title: Setlist notes\nstatus: draft",
            "status: draft\ntitle: Setlist notes"
        )
    );
    assert!(!a.unsynced() && !b.unsynced());

    // ── Offline, both of them ────────────────────────────────────────
    let at_a = vault_doc::render(a.doc().loro(), Some(original));
    edit(
        &a,
        &at_a,
        "---\ntitle: Setlist notes\nstatus: ready\n---\nOpen with the ballad.\nClose on the anthem.\n",
    );
    let at_b = vault_doc::render(b.doc().loro(), Some(original));
    edit(
        &b,
        &at_b,
        "---\ntitle: Sunday setlist\nstatus: draft\n---\nSoundcheck at four.\nOpen with the ballad.\n",
    );
    assert!(
        a.unsynced() && b.unsynced(),
        "an offline edit is not synced"
    );

    // A restarts before the connection comes back.
    let acked = a.acked().clone();
    persisted(&st_a, doc_id, 2).await;
    drop(a);
    let mut a = Replica::open(doc_id, st_a.clone(), Some(acked))
        .await
        .unwrap();
    assert!(a.unsynced(), "the restart kept the unsynced edit");
    assert!(render(&a).contains("Close on the anthem."));

    // ── Back online ──────────────────────────────────────────────────
    assert_eq!(sync_once(&url, &mut a, path).await, SessionEnd::Dropped);
    assert_eq!(sync_once(&url, &mut b, path).await, SessionEnd::Dropped);
    assert_eq!(sync_once(&url, &mut a, path).await, SessionEnd::Dropped);

    let want = "---\nstatus: ready\ntitle: Sunday setlist\n---\nSoundcheck at four.\nOpen with the ballad.\nClose on the anthem.\n";
    assert_eq!(render(&a), want);
    assert_eq!(render(&b), want);
    assert!(!a.unsynced() && !b.unsynced());

    // The server writes the file from the merged document — keeping the
    // file's own key order.
    let on_disk = support::org_root(&tmp).vault_dir().join(path);
    let file_want = "---\ntitle: Sunday setlist\nstatus: ready\n---\nSoundcheck at four.\nOpen with the ballad.\nClose on the anthem.\n";
    eventually("the mirror to hold the merge", async || {
        std::fs::read_to_string(&on_disk).ok().as_deref() == Some(file_want)
    })
    .await;
}

// t[verify offline.doc.create] — a note made with no server is a
// document at once, under a device-chosen path; on first sync the server
// registers the path and writes the file
#[tokio::test(flavor = "multi_thread")]
async fn a_document_created_offline_gets_its_file() {
    let (url, tmp) = support::boot_ws().await.expect("boot");
    let path = "Untitled 7c1e.md";
    let doc_id = vault_proto::collab_doc_id(VAULT, path);
    let dir = tempfile::tempdir().unwrap();
    let mut r = Replica::open(doc_id, store(dir.path()), None)
        .await
        .unwrap();
    // What `doc_replica::create_local_note` does, then the person types.
    let id = Uuid::new_v4().to_string();
    vault_doc::doc::set_prop(
        r.doc().loro(),
        vault_doc::ID_PROP,
        LoroValue::from(id.clone()),
    );
    vault_doc::doc::body_text(r.doc().loro())
        .insert(0, "Ideas from the train.\n")
        .unwrap();
    vault_doc::commit_as(r.doc().loro(), "created on this device");
    assert!(r.unsynced());

    assert_eq!(sync_once(&url, &mut r, path).await, SessionEnd::Dropped);
    assert!(!r.unsynced());
    let on_disk = support::org_root(&tmp).vault_dir().join(path);
    let want = format!("---\nid: {id}\n---\nIdeas from the train.\n");
    eventually("the server to write the new file", async || {
        std::fs::read_to_string(&on_disk).ok().as_deref() == Some(want.as_str())
    })
    .await;
}

// t[verify offline.doc.materialise] — a file edited on disk (an editor,
// an agent) merges into the document as a change and reaches a device
#[tokio::test(flavor = "multi_thread")]
async fn a_file_edited_on_disk_merges_into_the_document() {
    // The edit is made on disk, so the server's watcher has to be up.
    let (url, tmp) = support::boot_ws_watching().await.expect("boot");
    let vault: VaultSyncClient = lane(&url).await;
    let path = "Offline/Disk edit.md";
    vault
        .put_file(
            VAULT.into(),
            path.into(),
            b"---\ntitle: Disk\n---\nline one\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let doc_id = vault_proto::collab_doc_id(VAULT, path);
    let dir = tempfile::tempdir().unwrap();
    let mut r = Replica::open(doc_id, store(dir.path()), None)
        .await
        .unwrap();
    assert_eq!(sync_once(&url, &mut r, path).await, SessionEnd::Dropped);

    // Offline on the device: a line typed.
    let base = vault_doc::render(r.doc().loro(), None);
    edit(
        &r,
        &base,
        &base.replace("line one\n", "line one\nfrom the device\n"),
    );

    // Meanwhile on the server's disk: someone retitles and appends.
    let on_disk = support::org_root(&tmp).vault_dir().join(path);
    std::fs::write(
        &on_disk,
        "---\ntitle: Disk (edited)\n---\nline one\nfrom disk\n",
    )
    .unwrap();
    // Let the watcher fold it into the server's document.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert_eq!(sync_once(&url, &mut r, path).await, SessionEnd::Dropped);
    let got = render(&r);
    assert!(got.contains("title: Disk (edited)"), "{got}");
    assert!(
        got.contains("from the device") && got.contains("from disk"),
        "{got}"
    );
    eventually("the mirror to carry both", async || {
        std::fs::read_to_string(&on_disk)
            .is_ok_and(|t| t.contains("from the device") && t.contains("from disk"))
    })
    .await;
}

// t[verify offline.doc.access] — the server checks every attach: a
// document whose copy is not a version of the server's is refused, and
// one the server will not admit is refused — both stay on the device,
// unchanged, never dropped
#[tokio::test(flavor = "multi_thread")]
async fn refused_documents_stay_on_the_device() {
    let (url, _tmp) = support::boot_ws().await.expect("boot");
    let vault: VaultSyncClient = lane(&url).await;
    let path = "Offline/Lineage.md";
    vault
        .put_file(
            VAULT.into(),
            path.into(),
            b"server text\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let doc_id = vault_proto::collab_doc_id(VAULT, path);

    // A replica with its own history for the same id (the server's
    // document was recreated since this device last synced).
    let dir = tempfile::tempdir().unwrap();
    let mut r = Replica::open(doc_id, store(dir.path()), None)
        .await
        .unwrap();
    vault_doc::adopt_text(r.doc().loro(), "device text\n");
    r.doc().loro().commit();
    assert_eq!(
        sync_once(&url, &mut r, path).await,
        SessionEnd::ForeignLineage
    );
    assert_eq!(
        render(&r),
        "device text\n",
        "a refusal never changes the copy"
    );
    assert!(r.unsynced());

    // A path the server will not admit (outside the vault): refused at
    // registration, and the attach itself is refused too.
    let bad = "../outside.md";
    let mut stray = Replica::open(
        vault_proto::collab_doc_id(VAULT, bad),
        store(dir.path()),
        None,
    )
    .await
    .unwrap();
    vault_doc::adopt_text(stray.doc().loro(), "kept\n");
    stray.doc().loro().commit();
    assert!(matches!(
        sync_once(&url, &mut stray, bad).await,
        SessionEnd::Refused(_)
    ));
    assert!(matches!(
        sync_registered(&url, &mut stray).await,
        SessionEnd::Refused(_)
    ));
    assert_eq!(render(&stray), "kept\n");
}
