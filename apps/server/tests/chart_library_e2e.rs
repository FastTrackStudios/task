#![allow(clippy::large_futures)]
//! End-to-end check for the chart lane — and for the claim ADR 0003
//! rests on: *nothing new is built for libraries*.
//!
//! Keyflow keeps a person's charts in Task by calling four ordinary
//! RPCs (`upsert_chart` / `chart` / `list_charts` / `delete_chart`) on
//! the resources tier, and a library of those charts is an existing
//! `Collection` of kind `Library` over `chart:<slug>` node references.
//! No chart service, no chart store, no chart lane — which is exactly
//! what this test asserts by using only what was already mounted.
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
        updated_at: "2026-09-05T10:00:00Z".into(),
    }
}

const HOSANNA: &str = "[Verse 1]\n| A | E | F#m | D |\n\n[Chorus]\n| D | A | E |\n";
const DOXOLOGY: &str = "[Verse]\n| G | C | D | G |\n";

/// Two charts saved, listed, read back verbatim, gathered into a
/// `Library`, and one deleted out from under it.
#[tokio::test(flavor = "multi_thread")]
async fn charts_round_trip_and_a_library_collects_them() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let charts: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    // Two charts, saved the way Keyflow saves them.
    let first = charts
        .upsert_chart(chart("Hosanna", HOSANNA, &["verse-1", "chorus"]))
        .await
        .unwrap();
    assert_eq!(first.slug, "hosanna");
    assert_eq!(first.rel_path, "charts/hosanna.md");
    assert!(first.created);
    let second = charts
        .upsert_chart(chart("Doxology", DOXOLOGY, &["verse"]))
        .await
        .unwrap();
    assert_eq!(second.slug, "doxology");

    // On disk: the source is a plain chart file an outside editor
    // opens, and the manifest is an ordinary `type: resource` page.
    let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
    let dir = org_root.resources_dir().join("charts");
    assert_eq!(
        std::fs::read_to_string(dir.join("hosanna.kf")).unwrap(),
        HOSANNA,
        "the .kf holds the source verbatim"
    );
    let manifest = std::fs::read_to_string(dir.join("hosanna.md")).unwrap();
    assert!(manifest.contains("resource_kind: chart"), "{manifest}");
    assert!(manifest.contains("- chorus"), "sections: {manifest}");

    // Both list, and the source round-trips byte for byte.
    let list = charts.list_charts().await.unwrap();
    let slugs: Vec<&str> = list.iter().map(|c| c.slug.as_str()).collect();
    assert_eq!(slugs, ["doxology", "hosanna"], "slug order");
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
        assert!(!dir.join("hosanna.kf").exists());
        assert!(!dir.join("hosanna.md").exists());
        assert!(
            charts.chart("hosanna".to_owned()).await.is_err(),
            "the chart is gone"
        );
        assert_eq!(
            charts.list_charts().await.unwrap().len(),
            1,
            "only doxology remains"
        );

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
/// source is replaced outright, and a hand edit to the manifest body
/// survives — the same contract the sermon sync holds.
#[tokio::test(flavor = "multi_thread")]
async fn re_saving_a_chart_keeps_the_manifest_body() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let charts: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    charts
        .upsert_chart(chart("Great Are You Lord", HOSANNA, &["verse-1"]))
        .await
        .unwrap();

    let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
    let md = org_root
        .resources_dir()
        .join("charts/great-are-you-lord.md");
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
    assert!(charts.upsert_chart(chart("", "| A |", &[])).await.is_err());
    assert_eq!(charts.list_charts().await.unwrap().len(), 1);
}
