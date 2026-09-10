#![allow(clippy::large_futures)]
//! **Two people editing one chart converge.** ADR 0004 decision 1, and
//! the only assertion that can justify it.
//!
//! Everything else in the change is plumbing: a directory name, a
//! frontmatter key, a migration. The claim the plumbing exists to make
//! is that a chart *inherits* collaboration by living on a **registered
//! shelf** rather than having any built for it — so the test that
//! matters is the one where a chart Keyflow saved is picked up by two
//! independent CRDT replicas, edited concurrently, and agreed on.
//!
//! The shelf is `assets:charts` — `<org>/assets/charts/`, a sibling of
//! the vault rather than a directory inside it. That distinction is
//! what this file quietly proves: nothing below mentions the vault, and
//! it converges anyway, because collaboration follows registration and
//! not location (`org_proto::shelf`). An earlier draft filed charts
//! inside the vault believing the opposite, and paid for the belief by
//! making a cross-organisation chart library impossible.
//!
//! # Why this drives three different lanes at once
//!
//! Because the claim is exactly that they are the same file:
//!
//! 1. `ResourcesService::upsert_chart` — Keyflow's lane. Writes the
//!    document through `VaultSync::put_file`, which is what makes the
//!    collab layer see it at all.
//! 2. `VaultSync::open_collab` — the vault lane. Turns
//!    `(vault_id, path)` into a doc id. That the path it takes is the
//!    `rel_path` the *chart* lane handed back, unmodified, is the whole
//!    "an app addresses a chart without guessing" story.
//! 3. `DocSync` — the CRDT lane, over the registry `vault-collab`
//!    owns. Two `SyncedDoc` clients, no chart-shaped anything.
//!
//! All three over one real WebSocket to a booted server, because a lane
//! that works because both halves happen to share a process is the
//! failure this suite exists to catch. `vault_collab_e2e.rs` is the
//! sibling that pins the same machinery for a plain note; this file is
//! the one that says a chart is a plain note.
//!
//! # And then Keyflow saves over the top
//!
//! The last stage is the case that would be a data-loss bug if the
//! design were wrong: an app writing the file while two people have it
//! open. It must merge rather than revert, which is `vault-collab`'s
//! stated inbound policy — an external `put_file` into an open doc is
//! folded in three-way at character level. That policy is why
//! `AssetShelf::put` can use `IfMatch::Force`, and why
//! `chart::refresh_document` rewrites the smallest region it can.

#[allow(dead_code)]
mod support;

use std::time::Duration;

use crdt::CrdtDoc;
use crdt::sync::{DocSyncClient, SyncedDoc};
use resources_proto::{ChartDoc, ResourcesServiceClient};
use uuid::Uuid;
use vault_proto::{COLLAB_TEXT_CONTAINER, VaultSyncClient};

const SOURCE: &str = "[Verse]\n| G | C | D | G |\n";

fn chart(title: &str) -> ChartDoc {
    ChartDoc {
        slug: String::new(),
        title: title.into(),
        source: SOURCE.into(),
        key: "G".into(),
        notation: "keyflow".into(),
        sections: vec!["verse".into()],
        song: String::new(),
        arrangement: String::new(),
        is_default: false,
        updated_at: "2026-09-09T10:00:00Z".into(),
    }
}

/// A collaborating replica: an ephemeral local doc plus a sync session
/// against the served registry. Exactly what a second browser tab is.
async fn join(url: &str, doc_id: Uuid) -> CrdtDoc {
    let doc = CrdtDoc::ephemeral();
    let mut synced = SyncedDoc::new(doc_id, doc.clone());
    let client: DocSyncClient = vox::connect_lane(url).establish().await.unwrap();
    tokio::spawn(async move {
        let _ = synced.run(&client).await;
    });
    doc
}

fn text_of(doc: &CrdtDoc) -> String {
    doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string()
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

// t[verify storage.crdt.layer] — a chart is a shelf document, so two
// clients editing one converge through the same registry two clients
// editing a note do, and nothing chart-shaped was built to make it so
#[tokio::test(flavor = "multi_thread")]
async fn two_clients_editing_one_chart_converge() {
    let (url, tmp) = support::boot_ws().await.unwrap();
    let charts: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    let vault: VaultSyncClient = vox::connect_lane(&url).establish().await.unwrap();

    // ── Keyflow saves a chart ────────────────────────────────────────
    let saved = charts.upsert_chart(chart("Doxology")).await.unwrap();
    assert_eq!(
        saved.rel_path,
        resources_proto::assets::chart_path("doxology"),
        "the lane hands back a shelf path, so nobody has to guess one"
    );

    // ── and the path it handed back opens a collaborative document ───
    //
    // No translation step. This is the sentence ADR 0004 is about: the
    // chart lane's `rel_path` is a shelf path, and the vault lane takes
    // `(vault_id, path)` for *any* registered shelf — which is the
    // whole of why the shelf did not have to be inside the vault.
    let ack = vault
        .open_collab(
            resources_proto::assets::charts_vault_id(),
            saved.rel_path.clone(),
        )
        .await
        .expect("a chart is on a registered shelf, so it has a collab document");
    let doc_id = ack.doc_id;

    let alice = join(&url, doc_id).await;
    let bob = join(&url, doc_id).await;

    // Both replicas seed from the file Keyflow wrote — including the
    // chart source, because the source is in the document.
    for (who, doc) in [("alice", &alice), ("bob", &bob)] {
        eventually(&format!("{who} to seed from the saved chart"), async || {
            text_of(doc).contains(SOURCE)
        })
        .await;
    }

    // ── two people type at once ──────────────────────────────────────
    //
    // Alice adds a line to the notes; Bob adds a bar to the chart
    // itself. Different regions of one document, which is the ordinary
    // shape of two people working on a chart and the shape ADR 0003
    // could not express at all.
    let alice_note = "\n- capo 2 for Sunday\n";
    alice
        .loro()
        .get_text(COLLAB_TEXT_CONTAINER)
        .push_str(alice_note)
        .unwrap();
    alice.loro().commit();

    let bob_bar = "| Em | C |\n";
    {
        let text = bob.loro().get_text(COLLAB_TEXT_CONTAINER);
        let at = text.to_string().find(SOURCE).expect("bob sees the source") + SOURCE.len();
        // Loro indexes text in unicode code points; the fixture is
        // ASCII so the byte offset is the same number.
        text.insert(at, bob_bar).unwrap();
    }
    bob.loro().commit();

    // ── and converge ─────────────────────────────────────────────────
    eventually("alice and bob to converge", async || {
        let (a, b) = (text_of(&alice), text_of(&bob));
        a == b && a.contains(alice_note) && a.contains(bob_bar)
    })
    .await;

    // The file follows, through the write-behind. Nobody asked it to:
    // the debounce fires, `put_file` commits, and the chart on disk is
    // what both people are looking at.
    let org_root = support::org_root(&tmp);
    let path = org_root
        .asset_shelf_dir(resources_proto::assets::CHARTS_KIND)
        .join(&saved.rel_path);
    eventually("the write-behind to reach the file", async || {
        std::fs::read_to_string(&path).is_ok_and(|t| t.contains(alice_note) && t.contains(bob_bar))
    })
    .await;

    // And the chart lane reads Bob's bar back as chart source, because
    // the fence is where the source lives and Bob typed inside it.
    let read = charts.chart("doxology".to_owned()).await.unwrap();
    assert!(
        read.source.contains(bob_bar),
        "a collaborator's edit is not visible to the app that owns the \
         chart: {:?}",
        read.source
    );

    // ── Keyflow saves over the top of two live editors ───────────────
    //
    // The inbound merge, in the one case that would be data loss if it
    // went the other way. Keyflow replaces the source outright — that
    // is its documented contract — and Alice's note, which Keyflow has
    // never heard of and does not own, must survive.
    let mut replaced = chart("Doxology");
    replaced.slug = "doxology".into();
    replaced.source = "[Verse]\n| A | D | E |\n".into();
    charts.upsert_chart(replaced).await.unwrap();

    eventually("the app's write to merge into both replicas", async || {
        let (a, b) = (text_of(&alice), text_of(&bob));
        a == b && a.contains("| A | D | E |")
    })
    .await;
    assert!(
        text_of(&alice).contains(alice_note),
        "an app's save reverted a person's live edit: {}",
        text_of(&alice)
    );
}
