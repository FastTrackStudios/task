//! Chapter — Keyflow keeps its charts here, and a library is a
//! collection.
//!
//! ADR 0003's first claim, driven the way the app that made it will
//! drive it: an outside application signs in with an ordinary token,
//! calls four ordinary RPCs on the resources tier, and gathers what it
//! saved into an existing `Collection` of kind `Library`. No chart
//! service, no chart store, no private lane — the chapter passes using
//! only what was already mounted, which is the whole of what "nothing
//! new is built for libraries" means.
//!
//! # Over the wire, because that is the claim
//!
//! `apps/server/tests/chart_library_e2e.rs` already drives these lanes
//! over a WebSocket against a booted server, and the backends have unit
//! tests. What neither can say is whether a *client* — someone holding
//! a session token, dialling an org by its endpoint id, gated by the
//! permit table — reaches them. Twelve permit rows landed with these
//! lanes, and a row that is missing fails closed only for a caller the
//! gate actually evaluates.
//!
//! So everything here goes through [`integration::client::Session`] as
//! Alice. The one disk assertion is deliberate and is about the
//! opposite: what an editor that has never heard of Task sees when it
//! opens the folder.

use collection_proto::{CollectionKind, Placement};
use integration::scenario::Scenario;
use links_proto::{NodeKind, NodeRef};
use resources_proto::ChartDoc;

const HOSANNA: &str = "[Verse 1]\n| A | E | F#m | D |\n\n[Chorus]\n| D | A | E |\n";
const DOXOLOGY: &str = "[Verse]\n| G | C | D | G |\n";
const BE_THOU: &str = "[Verse]\n| Eb | Bb | Cm | Ab |\n";

/// A chart as Keyflow hands one over: the source verbatim, the sections
/// declared rather than parsed (the server reads nobody's source).
fn chart(title: &str, source: &str, sections: &[&str]) -> ChartDoc {
    ChartDoc {
        slug: String::new(),
        title: title.into(),
        source: source.into(),
        key: "A".into(),
        notation: "keyflow".into(),
        sections: sections.iter().map(|s| (*s).to_owned()).collect(),
        updated_at: "2026-09-06T10:00:00Z".into(),
    }
}

fn chart_ref(slug: &str) -> NodeRef {
    NodeRef::new(NodeKind::Chart, slug)
}

/// Three charts saved by a client, listed, read back verbatim, and
/// gathered into a `Library` that is then reordered and thinned.
#[tokio::test]
async fn a_client_keeps_a_chart_library_through_the_lanes_that_exist() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let org = s.orgs.acme.slug.clone();

    // ── Keyflow saves ────────────────────────────────────────────────
    let saved = alice
        .resources()
        .await
        .upsert_chart(chart("Hosanna", HOSANNA, &["verse-1", "chorus"]))
        .await
        .expect("save a chart");
    assert_eq!(saved.slug, "hosanna", "the slug is the chart's identity");
    assert_eq!(saved.rel_path, "charts/hosanna.md");
    assert!(saved.created);

    for (title, source, sections) in [
        ("Doxology", DOXOLOGY, &["verse"][..]),
        ("Be Thou My Vision", BE_THOU, &["verse"][..]),
    ] {
        alice
            .resources()
            .await
            .upsert_chart(chart(title, source, sections))
            .await
            .unwrap_or_else(|e| panic!("save {title}: {e:?}"));
    }

    // ── and reads them back ──────────────────────────────────────────
    let listed = alice
        .resources()
        .await
        .list_charts()
        .await
        .expect("list the charts");
    let slugs: Vec<&str> = listed.iter().map(|c| c.slug.as_str()).collect();
    for slug in ["hosanna", "doxology", "be-thou-my-vision"] {
        assert!(slugs.contains(&slug), "`{slug}` is missing from {slugs:?}");
    }

    let read = alice
        .resources()
        .await
        .chart("hosanna".into())
        .await
        .expect("read one chart");
    assert_eq!(
        read.source, HOSANNA,
        "the source came back changed — Keyflow's bytes are stored verbatim"
    );
    assert_eq!(
        read.sections,
        vec!["verse-1".to_owned(), "chorus".to_owned()],
        "the sections are the anchors `chart:hosanna#chorus` addresses"
    );

    // The one thing a client cannot see: on disk the source is a plain
    // chart file, so an editor that has never heard of Task opens it.
    let kf = s.orgs.acme.org_root().join("resources/charts/hosanna.kf");
    assert_eq!(
        std::fs::read_to_string(&kf).expect("the `.kf` is on disk"),
        HOSANNA,
        "the chart is stored as an encoding of a chart rather than as one"
    );

    // ── a library is a collection, and nothing else ──────────────────
    let library = alice
        .collections()
        .await
        .create(org.clone(), "Sunday Charts".into(), CollectionKind::Library)
        .await
        .expect("create the library");
    assert!(library.items.is_empty());

    for slug in ["hosanna", "doxology", "be-thou-my-vision"] {
        alice
            .collections()
            .await
            .add_item(Placement {
                collection_id: library.id.clone(),
                node: chart_ref(slug),
                after: None,
            })
            .await
            .unwrap_or_else(|e| panic!("collect `{slug}`: {e:?}"));
    }

    let ids = |c: &collection_proto::Collection| -> Vec<String> {
        c.items.iter().map(|i| i.node.id.clone()).collect()
    };

    let full = alice
        .collections()
        .await
        .get(library.id.clone())
        .await
        .expect("read the library")
        .expect("it is there");
    assert_eq!(
        ids(&full),
        ["hosanna", "doxology", "be-thou-my-vision"],
        "appending kept the order they were added in"
    );

    // A setlist is reordered by dragging one item, and only that item's
    // rank moves.
    let reordered = alice
        .collections()
        .await
        .reorder(Placement {
            collection_id: library.id.clone(),
            node: chart_ref("be-thou-my-vision"),
            after: Some(chart_ref("hosanna")),
        })
        .await
        .expect("reorder");
    assert_eq!(
        ids(&reordered),
        ["hosanna", "be-thou-my-vision", "doxology"],
        "the moved chart did not land where it was dropped"
    );

    let thinned = alice
        .collections()
        .await
        .remove_item(library.id.clone(), chart_ref("doxology"))
        .await
        .expect("remove one");
    assert_eq!(ids(&thinned), ["hosanna", "be-thou-my-vision"]);

    // And the library is findable as one, by kind — which is how a
    // client that holds no id opens it.
    let libraries = alice
        .collections()
        .await
        .list(org, Some(CollectionKind::Library))
        .await
        .expect("list the libraries");
    let found = libraries
        .iter()
        .find(|c| c.id == library.id)
        .expect("the library we made is listed as one");
    assert_eq!(ids(found), ["hosanna", "be-thou-my-vision"]);
}

/// Deleting a chart does not reach into the collections that reference
/// it. The reference stays, and reads as a reference to something that
/// is not there — a legible state, not an error (ADR 0003).
#[tokio::test]
async fn deleting_a_chart_leaves_the_reference_that_named_it() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let org = s.orgs.acme.slug.clone();

    alice
        .resources()
        .await
        .upsert_chart(chart("Hosanna", HOSANNA, &["chorus"]))
        .await
        .expect("save a chart");

    let library = alice
        .collections()
        .await
        .create(org, "Sunday Charts".into(), CollectionKind::Library)
        .await
        .expect("create the library");
    alice
        .collections()
        .await
        .add_item(Placement {
            collection_id: library.id.clone(),
            node: chart_ref("hosanna"),
            after: None,
        })
        .await
        .expect("collect it");

    assert!(
        alice
            .resources()
            .await
            .delete_chart("hosanna".into())
            .await
            .expect("delete"),
        "there was a chart to delete"
    );

    let after = alice
        .collections()
        .await
        .get(library.id)
        .await
        .expect("read the library")
        .expect("it is still there");
    assert_eq!(
        after.items.len(),
        1,
        "deleting a chart silently edited a collection that referenced it"
    );
    assert_eq!(after.items[0].node, chart_ref("hosanna"));

    // The chart itself is gone from both of its files.
    let charts = s.orgs.acme.org_root().join("resources/charts");
    assert!(!charts.join("hosanna.kf").exists());
    assert!(!charts.join("hosanna.md").exists());
}
