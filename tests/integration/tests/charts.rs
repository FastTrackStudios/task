//! Chapter — Keyflow keeps its charts here, they are ordinary vault
//! documents, and a library is a collection.
//!
//! ADR 0003's first claim, driven the way the app that made it will
//! drive it: an outside application signs in with an ordinary token,
//! calls four ordinary RPCs, and gathers what it saved into an existing
//! `Collection` of kind `Library`. No chart service, no chart store, no
//! private lane — the chapter passes using only what was already
//! mounted, which is the whole of what "nothing new is built for
//! libraries" means.
//!
//! ADR 0004 decision 1 then moved where the bytes land: a chart is a
//! **shelf document** on the `charts` asset group, and a song is one on
//! it. That the four RPCs did not change is what this chapter still
//! passing says. What it gained is the second half — the same client,
//! through the *vault* lane, seeing the chart it just saved as an
//! ordinary page. That is the claim ADR 0004 exists for, and a chapter
//! that only exercised `ResourcesService` could not make it.
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
        song: String::new(),
        arrangement: String::new(),
        is_default: false,
        updated_at: "2026-09-06T10:00:00Z".into(),
    }
}

/// One arrangement of a song: the chart, the song it arranges, the
/// label that tells it from the song's others, and whether it asks to
/// be that song's main one.
fn arrangement(title: &str, song: &str, label: &str, is_default: bool) -> ChartDoc {
    ChartDoc {
        song: song.into(),
        arrangement: label.into(),
        is_default,
        ..chart(title, DOXOLOGY, &["verse"])
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
    assert_eq!(
        saved.rel_path,
        resources_proto::assets::chart_path("hosanna"),
        "the lane hands back a vault path an app opens without composing one"
    );
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
        .list_charts(String::new())
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

    // ── and the chart it saved is an ordinary vault page ─────────────
    //
    // The whole of ADR 0004 decision 1, from the client side: the same
    // session, the *vault* lane, the `rel_path` the chart lane handed
    // back, unmodified. A chart is in the folder index, so search, the
    // graph, backlinks and `[[wikilink]]` autocomplete all reach it; it
    // declares which shelf it is on, so a notes list can leave it
    // there; and it has a collaborative document, so two people can
    // edit it. None of that was built for charts — it is what every
    // vault file already had.
    let vault = alice.vault().await;
    // The charts shelf, not the vault: a chart is an ordinary
    // collaborative document of `assets:charts` (ADR 0004), which is a
    // sibling root and therefore one another org can subscribe to.
    let index = vault
        .folder_index(resources_proto::assets::charts_vault_id())
        .await
        .expect("the folder index");
    let page = index
        .pages
        .iter()
        .find(|p| p.path == saved.rel_path)
        .expect("the chart is a page of its shelf like any other");
    assert_eq!(page.page_type, resources_proto::assets::TYPE_ASSET);

    let bytes = vault
        .get_file(
            resources_proto::assets::charts_vault_id(),
            saved.rel_path.clone(),
        )
        .await
        .expect("read the chart through the vault lane");
    let text = String::from_utf8(bytes.0).expect("utf8");
    assert!(
        text.contains(HOSANNA),
        "the source is a fence in the document's body: {text}"
    );
    assert!(
        vault
            .open_collab(
                resources_proto::assets::charts_vault_id(),
                saved.rel_path.clone()
            )
            .await
            .is_ok(),
        "a chart has a collaborative document because its shelf is registered — \
         which is what confers collaboration, not being inside the vault"
    );

    // The disk assertion is now about the tier, not the file format:
    // nothing of this chart is on the resources tier any more.
    assert!(
        !s.orgs
            .acme
            .org_root()
            .join("resources/charts/hosanna.kf")
            .exists(),
        "the `.kf` sidecar came back — a file the vault walker never \
         collects is a file nobody can search, link or collaborate on"
    );

    // ── a library is a collection, and nothing else ──────────────────
    let library = alice
        .collections()
        .await
        .create(
            org.clone(),
            "Sunday Charts".into(),
            CollectionKind::new("library"),
        )
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
        .list(org, Some(CollectionKind::new("library")))
        .await
        .expect("list the libraries");
    let found = libraries
        .iter()
        .find(|c| c.id == library.id)
        .expect("the library we made is listed as one");
    assert_eq!(ids(found), ["hosanna", "be-thou-my-vision"]);
}

/// A song played two ways is two charts, and the client never has to
/// work out which is the main one.
///
/// This is the chapter for the sentence the feature exists for: *"one
/// chart is one arrangement — the original, and a condensed live
/// version, with a default that is the main one."* Everything here goes
/// through the session as Alice, because the claim is about what a
/// Keyflow client reaches, not about what a backend can be made to do.
///
/// The part worth driving over the wire rather than in a unit test is
/// the **invariant**: the flag is the server's, so a client that saves
/// two arrangements without thinking about defaults still ends up with
/// a song that has exactly one.
#[tokio::test]
async fn a_song_carries_arrangements_and_exactly_one_of_them_is_the_default() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let defaults = |list: &[resources_proto::ChartSummary]| -> Vec<String> {
        list.iter()
            .filter(|c| c.is_default)
            .map(|c| c.slug.clone())
            .collect()
    };

    // Keyflow saves the album arrangement, asking for nothing.
    let original = alice
        .resources()
        .await
        .upsert_chart(arrangement("Doxology", "doxology", "original", false))
        .await
        .expect("save the first arrangement");
    assert_eq!(
        original.slug, "doxology-original",
        "the arrangement names the slug"
    );

    // And then the condensed live cut. Same song, different reading —
    // a second chart, not an edit of the first.
    let live = alice
        .resources()
        .await
        .upsert_chart(arrangement(
            "Doxology",
            "song:doxology",
            "condensed live",
            false,
        ))
        .await
        .expect("save the second arrangement");
    assert_eq!(
        live.slug, "doxology-condensed-live",
        "a song's second chart is named for what it is, not `doxology-2`"
    );

    // One call is the whole song screen: both arrangements, their
    // labels, and which one to open by default.
    let of_song = alice
        .resources()
        .await
        .list_charts("doxology".into())
        .await
        .expect("list the song's arrangements");
    assert_eq!(of_song.len(), 2, "{of_song:?}");
    assert!(
        of_song.iter().all(|c| c.song == "song:doxology"),
        "a bare slug is read as this org's own song: {of_song:?}"
    );
    assert_eq!(
        defaults(&of_song),
        std::slice::from_ref(&original.slug),
        "the song's first chart is its default even though nobody asked"
    );

    // Making the live cut the main one clears the other in the same
    // operation — the client says which, never both.
    alice
        .resources()
        .await
        .upsert_chart(ChartDoc {
            slug: live.slug.clone(),
            ..arrangement("Doxology", "song:doxology", "condensed live", true)
        })
        .await
        .expect("promote the live cut");
    let promoted = alice
        .resources()
        .await
        .list_charts("song:doxology".into())
        .await
        .expect("list again");
    assert_eq!(defaults(&promoted), std::slice::from_ref(&live.slug));

    // Deleting the default does not leave the song without one.
    assert!(
        alice
            .resources()
            .await
            .delete_chart(live.slug)
            .await
            .expect("delete the default"),
        "there was a chart to delete"
    );
    let remaining = alice
        .resources()
        .await
        .list_charts("song:doxology".into())
        .await
        .expect("list what is left");
    assert_eq!(
        defaults(&remaining),
        [original.slug],
        "deleting the default left the song with charts and no main one"
    );

    // A chart nobody has attached to a song is independent, and stays
    // out of every song's set — Keyflow saves one of these before the
    // person has said what song it is.
    alice
        .resources()
        .await
        .upsert_chart(arrangement("Untitled Sketch", "", "", true))
        .await
        .expect("save an unattached chart");
    let unattached = alice
        .resources()
        .await
        .list_charts(String::new())
        .await
        .expect("list everything")
        .into_iter()
        .find(|c| c.slug == "untitled-sketch")
        .expect("it is listed");
    assert!(
        unattached.song.is_empty() && !unattached.is_default,
        "an unattached chart claimed a song's default flag: {unattached:?}"
    );
    assert_eq!(
        alice
            .resources()
            .await
            .list_charts("song:doxology".into())
            .await
            .expect("the song's set")
            .len(),
        1,
        "an unattached chart leaked into a song's arrangements"
    );
}

/// The planted world holds the same thing, so a demo user can reach it
/// without saving anything: Track One is charted twice, and the album
/// arrangement is the default.
#[tokio::test]
async fn the_seeded_song_has_two_arrangements_with_the_original_default() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let planted = alice
        .resources()
        .await
        .list_charts("song:track-one".into())
        .await
        .expect("the seeded song's arrangements");
    let mut labels: Vec<(&str, &str, bool)> = planted
        .iter()
        .map(|c| (c.slug.as_str(), c.arrangement.as_str(), c.is_default))
        .collect();
    labels.sort_unstable();
    assert_eq!(
        labels,
        [
            ("track-one", "original", true),
            ("track-one-condensed-live", "condensed live", false),
        ],
        "the seed no longer plants two arrangements of one song"
    );
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
        .create(org, "Sunday Charts".into(), CollectionKind::new("library"))
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

/// A song, its two arrangements, and the join between them — ADR 0004's
/// other half, through the lanes a client actually holds.
///
/// The claim under test is that **nothing lists anything twice**. A
/// chart names its song; the song says nothing back. So adding an
/// arrangement is one write, and the answer to "what arrangements does
/// this song have?" is a query rather than a field somebody has to
/// remember to update. The vendored song folder this replaced kept the
/// same fact in four places — an `arrangements:` list, a
/// `defaultArrangement` uuid, a directory per arrangement, and each
/// arrangement's own note — and four places is four places to drift.
#[tokio::test]
async fn a_song_and_its_arrangements_join_by_reference_and_nothing_lists_twice() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let song = alice
        .resources()
        .await
        .upsert_song(resources_proto::SongDoc {
            slug: String::new(),
            title: "Opening Night".into(),
            writers: vec!["A. Wright".into()],
            key: "C Major".into(),
            tags: vec!["album".into()],
            updated_at: "2026-09-09T10:00:00Z".into(),
        })
        .await
        .expect("save a song");
    assert_eq!(song.slug, "opening-night");
    assert_eq!(
        song.rel_path,
        resources_proto::assets::song_path("opening-night"),
        "a song is a shelf document beside its charts, on its own shelf"
    );

    // Two arrangements, saved as two charts that each name the song.
    // Neither asks to be the default. The server still gives the song
    // exactly one, because a song with a chart and no main one is a
    // state nothing can render.
    for label in ["original", "live"] {
        alice
            .resources()
            .await
            .upsert_chart(arrangement(
                "Opening Night",
                "song:opening-night",
                label,
                false,
            ))
            .await
            .unwrap_or_else(|e| panic!("save the {label} arrangement: {e:?}"));
    }

    // The join, in one call and from the chart side only.
    let arrangements = alice
        .resources()
        .await
        .list_charts("song:opening-night".into())
        .await
        .expect("list the song's arrangements");
    let slugs: Vec<&str> = arrangements.iter().map(|c| c.slug.as_str()).collect();
    assert_eq!(
        slugs,
        ["opening-night-live", "opening-night-original"],
        "the arrangements are the charts that name the song, in slug order"
    );
    let defaults: Vec<&str> = arrangements
        .iter()
        .filter(|c| c.is_default)
        .map(|c| c.slug.as_str())
        .collect();
    assert_eq!(
        defaults.len(),
        1,
        "a song with charts has exactly one default, and the server owns \
         the flag: {arrangements:?}"
    );

    // And the song document itself lists none of them — the fact lives
    // in one place, on the side that can maintain it with one write.
    let vault = alice.vault().await;
    let bytes = vault
        .get_file(
            resources_proto::assets::songs_vault_id(),
            song.rel_path.clone(),
        )
        .await
        .expect("read the song through the vault lane");
    let text = String::from_utf8(bytes.0).expect("utf8");
    assert!(
        !text.contains("opening-night-live"),
        "the song document lists an arrangement — the join is one-way on \
         purpose: {text}"
    );
    assert!(
        !text.contains("defaultArrangement"),
        "a uuid pointer came back; the flag belongs on the chart: {text}"
    );

    // A song is a page of its shelf like any other, which is the whole
    // reason it moved.
    let index = vault
        .folder_index(resources_proto::assets::songs_vault_id())
        .await
        .expect("the folder index");
    let page = index
        .pages
        .iter()
        .find(|p| p.path == song.rel_path)
        .expect("the song is a page of its shelf");
    assert_eq!(page.page_type, resources_proto::assets::TYPE_ASSET);
    assert!(
        page.tags.contains(&"album".to_owned()),
        "tags come free with being a vault file: {:?}",
        page.tags
    );

    // Deleting the song leaves its arrangements: an arrangement of a
    // song that is gone is a legible state, not a reason to destroy
    // somebody's chart (the same choice ADR 0003 made for a collection
    // holding a deleted chart).
    assert!(
        alice
            .resources()
            .await
            .delete_song("opening-night".into())
            .await
            .expect("delete the song")
    );
    assert_eq!(
        alice
            .resources()
            .await
            .list_charts("song:opening-night".into())
            .await
            .expect("the arrangements outlive the song")
            .len(),
        2
    );
}
