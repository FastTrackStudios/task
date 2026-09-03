#![allow(clippy::large_futures)]
//! A wiki page edited through the vault path, end to end over a live
//! `task-server`: every wiki root is also a vault root (`wiki:<slug>`),
//! so the vault editor — `VaultSync` files, per-file CRDT collab, the
//! link graph, the `changes` stream — works on wiki pages unchanged,
//! while the wiki `Pages` service keeps serving the same files.
//!
//! Boots over the example studio (see `support`), whose `acme-audio`
//! org plants the `music-theory` wiki with `Concepts/Modes.md`.

use std::time::Duration;

use crdt::CrdtDoc;
use crdt::sync::{DocSyncClient, SyncedDoc};
use vault_proto::{
    COLLAB_TEXT_CONTAINER, IfMatch, VaultChange, VaultGraphClient, VaultSyncClient, VaultSyncError,
};
use vox::VoxError;
use wiki_proto::service::events::EventsStreamClient;
use wiki_proto::service::pages::PagesClient;
use wiki_proto::service::registry::RegistryClient;
use wiki_proto::{WikiChange, WikiEvent};

mod support;

const WIKI: &str = "music-theory";
const WIKI_VAULT: &str = "wiki:music-theory";
/// A page the seed plants in the wiki.
const SEEDED: &str = "Concepts/Modes.md";

async fn boot() -> (String, tempfile::TempDir) {
    support::boot_ws().await.expect("boot")
}

async fn lane<C: vox_core::FromVoxLane>(url: &str) -> C {
    vox::connect_lane(url)
        .establish()
        .await
        .unwrap_or_else(|e| panic!("connect: {e:?}"))
}

/// Ten seconds of patience: the write-behind debounce plus a machine
/// running the rest of the suite beside this binary.
async fn eventually(what: &str, mut cond: impl AsyncFnMut() -> bool) {
    for _ in 0..400 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn text_of(doc: &CrdtDoc) -> String {
    doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string()
}

/// A wiki root answers as a vault: the seeded page is in the manifest,
/// a `put_file` over the wiki id is the same bytes (and sha) the wiki
/// `Pages` service reads back, the link graph answers for the wiki and
/// stays isolated from the org vault, and an unknown wiki is `NotFound`
/// rather than a directory that springs into being.
#[tokio::test(flavor = "multi_thread")]
async fn a_wiki_root_is_a_vault_root_the_pages_service_shares() {
    let (url, _tmp) = boot().await;
    let sync: VaultSyncClient = lane(&url).await;
    let graph: VaultGraphClient = lane(&url).await;
    let pages: PagesClient = lane(&url).await;

    // The seed is visible through the vault door.
    let manifest = sync.manifest(WIKI_VAULT.to_string()).await.unwrap();
    assert_eq!(manifest.vault_id, WIKI_VAULT);
    let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&SEEDED), "{paths:?}");
    for p in &paths {
        assert!(
            !p.starts_with('/') && !p.contains(".."),
            "wiki manifest path escapes its root: {p}"
        );
    }
    let seeded_via_vault = sync
        .get_file(WIKI_VAULT.to_string(), SEEDED.to_string())
        .await
        .unwrap();
    let seeded_via_wiki = pages
        .read_page(WIKI.to_string(), SEEDED.to_string())
        .await
        .unwrap();
    assert_eq!(seeded_via_vault.0, seeded_via_wiki.markdown.as_bytes());

    // An editor save (a put over the wiki id) is what the wiki reads.
    let body = "---\ntitle: Dorian\ntype: concept\n---\n\n# Dorian\n\nThe second of the [[Modes]]; see [[Ionian]].\n";
    let ack = sync
        .put_file(
            WIKI_VAULT.to_string(),
            "Concepts/Dorian.md".to_string(),
            body.as_bytes().to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let doc = pages
        .read_page(WIKI.to_string(), "Concepts/Dorian.md".to_string())
        .await
        .unwrap();
    assert_eq!(doc.markdown, body);
    assert_eq!(doc.sha256, ack.sha256, "one file, one sha, two doors");

    // The folder index the editor's wikilink completion uses.
    let idx = sync.folder_index(WIKI_VAULT.to_string()).await.unwrap();
    let dorian = idx
        .pages
        .iter()
        .find(|p| p.path == "Concepts/Dorian.md")
        .expect("indexed");
    assert_eq!(dorian.page_type, "concept");
    assert_eq!(dorian.sha256, ack.sha256);

    // The link graph resolves within the wiki…
    let back = graph
        .backlinks(WIKI_VAULT.to_string(), SEEDED.to_string())
        .await
        .unwrap();
    assert!(
        back.iter().any(|p| p == "Concepts/Dorian.md"),
        "wiki backlinks: {back:?}"
    );
    let out = graph
        .links(WIKI_VAULT.to_string(), "Concepts/Dorian.md".to_string())
        .await
        .unwrap();
    assert!(
        out.iter()
            .any(|l| l.linkpath == "Modes" && l.resolved.as_deref() == Some(SEEDED)),
        "wiki links: {out:?}"
    );
    // …and not across into the org vault, whose own manifest holds
    // its own pages and none of the wiki's.
    let vault_back = graph
        .backlinks("default".to_string(), SEEDED.to_string())
        .await
        .unwrap();
    assert!(
        vault_back.is_empty(),
        "leaked into the vault: {vault_back:?}"
    );
    let vault_manifest = sync.manifest("default".to_string()).await.unwrap();
    let vault_paths: Vec<&str> = vault_manifest
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert!(
        vault_paths.contains(&support::EXAMPLE_PAGE),
        "{vault_paths:?}"
    );
    assert!(
        !vault_paths.contains(&"Concepts/Dorian.md"),
        "a wiki page in the vault manifest: {vault_paths:?}"
    );

    // A wiki nobody created is not a vault.
    let missing = sync
        .get_file("wiki:nope".to_string(), SEEDED.to_string())
        .await;
    assert!(
        matches!(missing, Err(VoxError::User(ref e)) if matches!(**e, VaultSyncError::NotFound)),
        "{missing:?}"
    );

    // One boot serves the rest of the scenario: each of these is a
    // check of its own, sharing the server rather than paying a fresh
    // `AppState` (the slow part) per assertion.
    an_editor_save_reaches_the_wiki_events_stream(&url).await;
    vault_changes_carry_the_wiki_vault_id(&url).await;
    a_wiki_created_at_runtime_is_editable_at_once(&url).await;
}

/// A save through the vault door reaches the wiki's own event stream,
/// so the wiki home and sidebar (which listen there) stay live.
async fn an_editor_save_reaches_the_wiki_events_stream(url: &str) {
    let events: EventsStreamClient = lane(url).await;
    let sync: VaultSyncClient = lane(url).await;

    let (tx, mut rx) = vox::channel::<WikiChange>();
    let _sub = tokio::spawn(async move {
        let _ = events.changes(tx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    sync.put_file(
        WIKI_VAULT.to_string(),
        "Concepts/Lydian.md".to_string(),
        b"# Lydian\n".to_vec(),
        IfMatch::CreateOnly,
    )
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let msg = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("wiki event timeout")
            .expect("rx error")
            .expect("rx closed");
        let change = msg.get();
        if change.wiki_id == WIKI
            && matches!(&change.event, WikiEvent::PageWritten { path, .. } if path == "Concepts/Lydian.md")
        {
            break;
        }
    }
}

/// A wiki created while the server runs becomes a vault root without a
/// restart: the registry's `create_wiki` registers it, and the next
/// `put_file` over its id lands in its directory.
async fn a_wiki_created_at_runtime_is_editable_at_once(url: &str) {
    let registry: RegistryClient = lane(url).await;
    let sync: VaultSyncClient = lane(url).await;
    let pages: PagesClient = lane(url).await;

    let summary = registry
        .create_wiki(wiki_proto::config::NewWiki {
            title: "Field Notes".into(),
            slug: String::new(),
            purpose: "What we learn on location.".into(),
            visibility: wiki_proto::config::Visibility::Private,
            source: None,
        })
        .await
        .unwrap();
    assert_eq!(summary.slug, "field-notes");
    let vault_id = "wiki:field-notes".to_string();

    // Registration is handed off from the dispatcher thread; give it
    // the moment it needs rather than a restart.
    let s = sync.clone();
    let v = vault_id.clone();
    eventually("new wiki serves as a vault", async || {
        s.manifest(v.clone()).await.is_ok()
    })
    .await;

    sync.put_file(
        vault_id.clone(),
        "Notes/Day One.md".to_string(),
        b"# Day one\n".to_vec(),
        IfMatch::CreateOnly,
    )
    .await
    .unwrap();
    let doc = pages
        .read_page("field-notes".to_string(), "Notes/Day One.md".to_string())
        .await
        .unwrap();
    assert_eq!(doc.markdown, "# Day one\n");
}

/// Two collab sessions on a wiki page converge, the merged text lands
/// in the file the wiki `Pages` service reads, and a plain `put_file`
/// (the CLI, an ingest) merges into both open replicas — the same
/// guarantees `vault_collab_e2e` proves for the org vault, over a wiki
/// root, through the real WebSocket mounts.
#[tokio::test(flavor = "multi_thread")]
async fn two_collab_sessions_converge_on_a_wiki_page() {
    let (url, _tmp) = boot().await;
    let writer: VaultSyncClient = lane(&url).await;
    let pages: PagesClient = lane(&url).await;
    let path = "Concepts/Collab.md".to_string();

    writer
        .put_file(
            WIKI_VAULT.to_string(),
            path.clone(),
            b"base\n".to_vec(),
            IfMatch::CreateOnly,
        )
        .await
        .unwrap();
    let ack = writer
        .open_collab(WIKI_VAULT.to_string(), path.clone())
        .await
        .unwrap();
    assert_eq!(
        ack.doc_id,
        vault_proto::collab_doc_id(WIKI_VAULT, &path),
        "the doc id is derived from the wiki's vault id"
    );

    // Two replicas over the served DocSync registry.
    let mut docs = Vec::new();
    for _ in 0..2 {
        let doc = CrdtDoc::ephemeral();
        let mut synced = SyncedDoc::new(ack.doc_id, doc.clone());
        let client: DocSyncClient = lane(&url).await;
        tokio::spawn(async move {
            let _ = synced.run(&client).await;
        });
        docs.push(doc);
    }
    let (doc_a, doc_b) = (docs[0].clone(), docs[1].clone());
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("A seeded", async || text_of(&a) == "base\n").await;
    eventually("B seeded", async || text_of(&b) == "base\n").await;

    // A types; B and the wiki's own reader observe it.
    doc_a
        .loro()
        .get_text(COLLAB_TEXT_CONTAINER)
        .insert(0, "alpha ")
        .unwrap();
    doc_a.loro().commit();
    let b = doc_b.clone();
    eventually("B sees A's edit", async || text_of(&b) == "alpha base\n").await;
    let (p, pth) = (pages.clone(), path.clone());
    eventually("write-behind reaches the wiki Pages service", async || {
        p.read_page(WIKI.to_string(), pth.clone())
            .await
            .map(|d| d.markdown == "alpha base\n")
            .unwrap_or(false)
    })
    .await;

    // B types too — both converge.
    doc_b
        .loro()
        .get_text(COLLAB_TEXT_CONTAINER)
        .insert(11, "beta\n")
        .unwrap();
    doc_b.loro().commit();
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("replicas converge", async || {
        let (ta, tb) = (text_of(&a), text_of(&b));
        ta == tb && ta.contains("alpha") && ta.contains("beta")
    })
    .await;

    // A non-CRDT writer (the CLI's `write_page` shape, here over the
    // vault wire) appends; the inbound listener merges it into both.
    let current = String::from_utf8(
        writer
            .get_file(WIKI_VAULT.to_string(), path.clone())
            .await
            .unwrap()
            .0,
    )
    .unwrap();
    writer
        .put_file(
            WIKI_VAULT.to_string(),
            path.clone(),
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

    // Quiesce: no echo re-applied, disk == replicas, one copy of each.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let final_a = text_of(&doc_a);
    assert_eq!(final_a, text_of(&doc_b), "replicas diverged");
    let on_disk = pages
        .read_page(WIKI.to_string(), path.clone())
        .await
        .unwrap()
        .markdown;
    assert_eq!(
        on_disk, final_a,
        "the wiki's file diverged from the replicas"
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

    // A `write_page` through the wiki door, while the doc is open,
    // merges the same way: the second door is not a bypass.
    let sha = pages
        .read_page(WIKI.to_string(), path.clone())
        .await
        .unwrap()
        .sha256;
    pages
        .write_page(
            WIKI.to_string(),
            path.clone(),
            format!("{final_a}from-wiki\n"),
            sha,
        )
        .await
        .unwrap();
    let (a, b) = (doc_a.clone(), doc_b.clone());
    eventually("replicas pick up a wiki write_page", async || {
        let (ta, tb) = (text_of(&a), text_of(&b));
        ta == tb && ta.contains("from-wiki")
    })
    .await;
}

/// The vault `changes` stream names the wiki's vault id, which is how a
/// client keeps the wiki it browses out of several.
async fn vault_changes_carry_the_wiki_vault_id(url: &str) {
    let stream: vault_proto::VaultSyncStreamClient = lane(url).await;
    let sync: VaultSyncClient = lane(url).await;
    let (tx, mut rx) = vox::channel::<VaultChange>();
    let _sub = tokio::spawn(async move {
        let _ = stream.changes(tx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    sync.put_file(
        WIKI_VAULT.to_string(),
        "Concepts/Mixolydian.md".to_string(),
        b"# Mixolydian\n".to_vec(),
        IfMatch::CreateOnly,
    )
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let msg = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("vault event timeout")
            .expect("rx error")
            .expect("rx closed");
        let change = msg.get();
        if let vault_proto::VaultEvent::Put { path, .. } = &change.event
            && path == "Concepts/Mixolydian.md"
        {
            assert_eq!(change.vault_id, WIKI_VAULT);
            break;
        }
    }
}
