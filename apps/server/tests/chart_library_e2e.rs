#![allow(clippy::large_futures)]
//! End-to-end check for the chart lane — and for the claim ADR 0003
//! rests on: *nothing new is built for libraries*.
//!
//! Keyflow keeps a person's charts in Task by calling four ordinary
//! RPCs (`upsert_chart` / `chart` / `list_charts` / `delete_chart`),
//! and a library of those charts is an existing `Collection` of kind
//! `Library` over `chart:<slug>` node references. No chart service, no
//! chart store, no chart lane — which is exactly what this test asserts
//! by using only what was already mounted.
//!
//! ADR 0004 moved where the bytes land: a chart is a **shelf document**
//! on the `Assets/` shelf, not a manifest-plus-`.kf` on the resources
//! tier. The lane did not change, which is the point of this file still
//! passing — an app calls the same four RPCs and gets back a `rel_path`
//! it can hand straight to `VaultSync`.
//!
//! It also pins the honest half of the decision: deleting a chart does
//! not reach into the collections that referenced it. The reference
//! stays, parses, and reads as unresolved — a legible state, not an
//! error.

// This binary uses the boot helpers only; the seed constants are for
// the vault suites.
#[allow(dead_code)]
mod support;

use resources_proto::{ChartDoc, ResourcesServiceClient};

fn chart(title: &str, source: &str, sections: &[&str]) -> ChartDoc {
    ChartDoc {
        slug: String::new(),
        title: title.into(),
        source: source.into(),
        key: "A".into(),
        notation: "keyflow".into(),
        sections: sections.iter().map(|s| (*s).to_string()).collect(),
        song: String::new(),
        arrangement: String::new(),
        is_default: false,
        updated_at: "2026-09-05T10:00:00Z".into(),
    }
}

/// One arrangement of a song, as Keyflow saves it: the chart, the song
/// it arranges, the label that tells it from the song's others, and
/// whether it is asking to be the main one.
fn arrangement(title: &str, song: &str, label: &str, is_default: bool) -> ChartDoc {
    ChartDoc {
        song: song.into(),
        arrangement: label.into(),
        is_default,
        ..chart(title, DOXOLOGY, &["verse"])
    }
}

const HOSANNA: &str = "[Verse 1]\n| A | E | F#m | D |\n\n[Chorus]\n| D | A | E |\n";
const DOXOLOGY: &str = "[Verse]\n| G | C | D | G |\n";

/// Two charts saved, listed, read back verbatim, gathered into a
/// `Library`, and one deleted out from under it.
#[tokio::test(flavor = "multi_thread")]
async fn charts_round_trip_and_a_library_collects_them() {
    let (url, tmp) = support::boot_ws().await.unwrap();
    let charts: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    // Two charts, saved the way Keyflow saves them.
    let first = charts
        .upsert_chart(chart("Hosanna", HOSANNA, &["verse-1", "chorus"]))
        .await
        .unwrap();
    assert_eq!(first.slug, "hosanna");
    assert_eq!(
        first.rel_path, "hosanna.md",
        "the path an app opens through VaultSync, relative to the \
         `assets:charts` shelf it names alongside"
    );
    assert!(first.created);
    let second = charts
        .upsert_chart(chart("Doxology", DOXOLOGY, &["verse"]))
        .await
        .unwrap();
    assert_eq!(second.slug, "doxology");

    // On disk: one markdown document on the charts shelf, declaring its
    // tier, with the source in its own body. Nothing on the resources
    // tier, and nothing under `vault/` either — that is the whole of
    // ADR 0004 decision 1, visible in a directory listing.
    let org_root = support::org_root(&tmp);
    let dir = org_root.asset_shelf_dir(resources_proto::assets::CHARTS_KIND);
    let document = std::fs::read_to_string(dir.join("hosanna.md")).unwrap();
    assert!(document.contains("type: asset"), "{document}");
    assert!(document.contains("asset_kind: chart"), "{document}");
    assert!(
        document.contains("sections: [verse-1, chorus]"),
        "sections are written in flow form, because the parser behind the \
         vault's folder index reads an unindented block sequence as empty \
         — see `resources::asset::inline_sequences`: {document}"
    );
    assert!(
        document.contains(HOSANNA),
        "the source is in the body, verbatim: {document}"
    );
    assert!(
        !org_root.resources_dir().join("charts/hosanna.kf").exists(),
        "the `.kf` sidecar is gone — a vault file the walker never \
         collects is a file nobody can search, link or collaborate on"
    );

    // Both list, and the source round-trips byte for byte. The seed
    // plants a chart of its own (`chart:track-one`), so this asserts
    // what it wrote is present and in slug order — not that nothing
    // else is there.
    let list = charts.list_charts(String::new()).await.unwrap();
    let slugs: Vec<&str> = list.iter().map(|c| c.slug.as_str()).collect();
    assert!(
        slugs.contains(&"hosanna") && slugs.contains(&"doxology"),
        "{slugs:?}"
    );
    let pos = |s: &str| slugs.iter().position(|c| *c == s).unwrap();
    assert!(pos("doxology") < pos("hosanna"), "slug order: {slugs:?}");
    let one = charts.chart("hosanna".to_owned()).await.unwrap();
    assert_eq!(one.source, HOSANNA);
    assert_eq!(one.key, "A");
    assert_eq!(one.sections, ["verse-1", "chorus"]);

    // A library is a Collection — no chart-shaped machinery anywhere.
    #[cfg(feature = "plugin-fasttrackstudio")]
    {
        use collection_proto::{CollectionKind, CollectionServiceClient, NodeKind, NodeRef};

        let node = |slug: &str| NodeRef::new(NodeKind::Chart, slug);
        let collections: CollectionServiceClient =
            vox::connect_lane(&url).establish().await.unwrap();
        let library = collections
            .create(
                support::ORG.to_owned(),
                "Sunday Charts".to_owned(),
                CollectionKind::Library,
            )
            .await
            .unwrap();
        for slug in ["hosanna", "doxology"] {
            collections
                .add_item(collection_proto::Placement {
                    collection_id: library.id.clone(),
                    node: node(slug),
                    after: None,
                })
                .await
                .unwrap();
        }

        let back = collections
            .get(library.id.clone())
            .await
            .unwrap()
            .expect("the library exists");
        assert_eq!(back.kind, CollectionKind::Library);
        let tokens: Vec<String> = back.items.iter().map(|i| i.node.to_token()).collect();
        assert_eq!(
            tokens,
            ["chart:hosanna", "chart:doxology"],
            "items come back in rank order, appended in the order added"
        );
        let ranks: Vec<&str> = back.items.iter().map(|i| i.rank.as_str()).collect();
        assert!(ranks[0] < ranks[1], "ranks ascend: {ranks:?}");

        // Delete a chart and the reference outlives it: the file is
        // gone, the collection still parses, and the item still names
        // the chart it wanted. Resolution is the reader's problem, and
        // an unresolved reference is not an error (ADR 0003).
        assert!(charts.delete_chart("hosanna".to_owned()).await.unwrap());
        assert!(!dir.join("hosanna.md").exists());
        assert!(
            charts.chart("hosanna".to_owned()).await.is_err(),
            "the chart is gone"
        );
        let after: Vec<String> = charts
            .list_charts(String::new())
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.slug)
            .collect();
        assert!(!after.contains(&"hosanna".to_owned()), "{after:?}");
        assert!(after.contains(&"doxology".to_owned()), "{after:?}");

        let dangling = collections
            .get(library.id.clone())
            .await
            .unwrap()
            .expect("the library survived the delete");
        let tokens: Vec<String> = dangling.items.iter().map(|i| i.node.to_token()).collect();
        assert_eq!(
            tokens,
            ["chart:hosanna", "chart:doxology"],
            "the dangling reference is kept, in order, and still parses"
        );
        assert!(
            dangling.items[0].node.is_local(),
            "a local reference names this org, not another"
        );
    }

    // Deleting twice is `false`, never an error — the same shape a
    // second Keyflow tab hitting delete would see.
    #[cfg(not(feature = "plugin-fasttrackstudio"))]
    assert!(charts.delete_chart("hosanna".to_owned()).await.unwrap());
    assert!(!charts.delete_chart("hosanna".to_owned()).await.unwrap());
}

/// The re-save path Keyflow lives on: the slug is the identity, the
/// source is replaced outright, and a hand edit to the document body
/// survives — the same contract the sermon sync holds, now covering the
/// prose beside a chart as well as the frontmatter around it.
///
/// This is the assertion collaboration rests on. A person typing under
/// `## Notes` and an app saving the chart are editing one document, and
/// the app has to rewrite the smallest region it can — its frontmatter
/// keys and its own fence — or every save would stamp on live work.
#[tokio::test(flavor = "multi_thread")]
async fn re_saving_a_chart_keeps_the_document_body() {
    let (url, tmp) = support::boot_ws().await.unwrap();
    let charts: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    charts
        .upsert_chart(chart("Great Are You Lord", HOSANNA, &["verse-1"]))
        .await
        .unwrap();

    let org_root = support::org_root(&tmp);
    let md = org_root
        .asset_shelf_dir(resources_proto::assets::CHARTS_KIND)
        .join(resources_proto::assets::chart_path("great-are-you-lord"));
    let hand = std::fs::read_to_string(&md)
        .unwrap()
        .replace("## Notes", "## Notes\n- capo 2 for Sunday\n");
    std::fs::write(&md, hand).unwrap();

    let mut edited = chart("Great Are You Lord", DOXOLOGY, &["verse-1", "chorus"]);
    edited.slug = "great-are-you-lord".into();
    edited.key = "G".into();
    let again = charts.upsert_chart(edited).await.unwrap();
    assert_eq!(again.slug, "great-are-you-lord");
    assert!(!again.created, "the slug is the identity");

    let text = std::fs::read_to_string(&md).unwrap();
    assert!(text.contains("- capo 2 for Sunday"), "body kept: {text}");
    assert!(text.contains("key: G"), "app-owned frontmatter: {text}");
    let doc = charts.chart("great-are-you-lord".to_owned()).await.unwrap();
    assert_eq!(doc.source, DOXOLOGY, "the source is replaced, not merged");
    assert_eq!(doc.sections, ["verse-1", "chorus"]);

    // An empty title is refused, and writes nothing.
    let before = charts.list_charts(String::new()).await.unwrap().len();
    assert!(charts.upsert_chart(chart("", "| A |", &[])).await.is_err());
    assert_eq!(
        charts.list_charts(String::new()).await.unwrap().len(),
        before
    );
}

/// Two arrangements of one song, over the wire: saved as two charts,
/// filtered back as one song's set, and the default moved between them
/// by the server rather than by whoever wrote last.
#[tokio::test(flavor = "multi_thread")]
async fn a_song_carries_several_arrangements_with_one_default() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let charts: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    // The first chart of the song. It asks for nothing, and is the
    // default anyway — a song with one chart and no main one is a state
    // nothing can render.
    let original = charts
        .upsert_chart(arrangement(
            "Be Thou My Vision",
            "be-thou",
            "original",
            false,
        ))
        .await
        .unwrap();
    assert_eq!(original.slug, "be-thou-my-vision-original");

    let live = charts
        .upsert_chart(arrangement(
            "Be Thou My Vision",
            "song:be-thou",
            "condensed live",
            false,
        ))
        .await
        .unwrap();
    assert_eq!(
        live.slug, "be-thou-my-vision-condensed-live",
        "the second arrangement is named for what it is, not `-2`"
    );

    // One call renders the song: its arrangements, their labels, and
    // which one is the main one.
    let of_song = charts.list_charts("be-thou".to_owned()).await.unwrap();
    assert_eq!(of_song.len(), 2, "{of_song:?}");
    assert!(
        of_song.iter().all(|c| c.song == "song:be-thou"),
        "a bare slug is normalised to this org's own song: {of_song:?}"
    );
    let default_of = |list: &[resources_proto::ChartSummary]| -> Vec<String> {
        list.iter()
            .filter(|c| c.is_default)
            .map(|c| c.slug.clone())
            .collect()
    };
    assert_eq!(default_of(&of_song), std::slice::from_ref(&original.slug));

    // Asking moves it, and clears the loser in the same operation.
    charts
        .upsert_chart(ChartDoc {
            slug: live.slug.clone(),
            ..arrangement("Be Thou My Vision", "song:be-thou", "condensed live", true)
        })
        .await
        .unwrap();
    let after = charts.list_charts("song:be-thou".to_owned()).await.unwrap();
    assert_eq!(default_of(&after), std::slice::from_ref(&live.slug));

    // Deleting the default promotes the oldest remaining, so the song
    // never has charts and no main one.
    assert!(charts.delete_chart(live.slug).await.unwrap());
    let left = charts.list_charts("song:be-thou".to_owned()).await.unwrap();
    assert_eq!(default_of(&left), [original.slug]);

    // The unfiltered list is still every chart the org holds — the seed
    // plants some of its own.
    let all = charts.list_charts(String::new()).await.unwrap();
    assert!(all.len() > left.len(), "{all:?}");
}

/// The seed's own chart, planted from the committed example tree and
/// read back through the RPC a client would use. The repo's policy is
/// that a feature lives in the suite *and* the seed; this is the half
/// that proves the second.
#[tokio::test(flavor = "multi_thread")]
async fn the_seeded_chart_is_readable_through_the_lane() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let charts: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    let doc = charts.chart("track-one".to_owned()).await.unwrap();
    assert_eq!(doc.title, "Track One");
    assert_eq!(doc.key, "A");
    assert_eq!(doc.sections, ["verse-1", "chorus", "bridge"]);
    assert!(
        doc.source.contains("[Chorus]"),
        "the source came out of the document's own fence"
    );

    // And the seeded chart is an ordinary page of its shelf: it is in
    // the folder index, so search, the graph and `[[wikilinks]]` all
    // reach it, and it is on a shelf of its own rather than in the
    // middle of somebody's notes. That sentence is the entire justification for ADR 0004
    // decision 1, and this is the only place it is checked over the
    // wire rather than asserted in prose.
    let vault: vault_proto::VaultSyncClient = vox::connect_lane(&url).establish().await.unwrap();
    let index = vault
        .folder_index(resources_proto::assets::charts_vault_id())
        .await
        .unwrap();
    let page = index
        .pages
        .iter()
        .find(|p| p.path == resources_proto::assets::chart_path("track-one"))
        .expect("the chart is a page of its shelf like any other");
    assert_eq!(page.page_type, resources_proto::assets::TYPE_ASSET);
    assert!(
        page.tags.contains(&"chart".to_owned()),
        "tags come free with being a vault file: {:?}",
        page.tags
    );
}
