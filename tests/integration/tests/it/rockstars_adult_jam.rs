#![allow(clippy::large_futures)]
//! Chapter — **Rockstars of Tomorrow runs a season**: the Adult Jam library
//! at its real size, setlists drawn from it, and shows made of setlists.
//!
//! `keyflow_library.rs` proves each move on a six-song library. This is the
//! library those moves are for: 204 charts from the jam program's
//! chordsheet.com backup, planted by the seed exactly as Keyflow would
//! have imported them (`example_org::CHORDSHEET_LIBRARIES`), all in one
//! song list called Adult Jam.
//!
//! # A show is a collection of setlists
//!
//! Nothing new was built for shows beyond one node kind. A show is a
//! collection whose items are `collection:<id>` references to setlists, in
//! running order — the same lexorank, the same `add_item`/`reorder`, the
//! same by-reference rule one level up. So the claims here are the same
//! claims made of setlists, and they are tested the same way, by change:
//! a setlist tightened after it went into two shows is tightened in both.
//!
//! The one invariant that nesting adds is that a collection can never come
//! to contain itself. It is refused where the edge would be made, and this
//! chapter makes the attempt over the wire.
//!
//! # Everything is Keyflow's own calls
//!
//! Reading the library goes through `integration::keyflow::Keyflow`, the
//! mirror of keyflow's `library/vox.rs`, signed in as Riley, who runs the
//! program.

use integration::client::Session;
use integration::keyflow::{Collections, Keyflow, collection, ids};
use integration::scenario::Scenario;
use integration::server::Server;
use links_proto::NodeRef;
use task_server::example_org::{CHORDSHEET_LIBRARIES, chordsheet_songs};

const ORG: &str = "rockstars-of-tomorrow";
const ADULT_JAM: &str = "Adult Jam";

/// Boot the program's org beside ACME — planted like every example org —
/// and sign Riley in.
async fn rockstars(s: &Scenario) -> (Server, Session) {
    let server = s
        .orgs
        .acme
        .start_beside("Rockstars of Tomorrow", ORG, |_| {})
        .await;
    let riley = integration::people::account(&server, "riley@rockstars.test", "Riley").await;
    let session = Session::open(&server, riley.token.clone()).await;
    (server, session)
}

/// The whole library is there, in the backup's order, and Keyflow opens
/// any song of it to the chart that came out of the backup.
#[tokio::test]
async fn the_adult_jam_library_is_all_there_and_opens_to_its_charts() {
    let s = Scenario::open().await;
    let (_server, riley) = rockstars(&s).await;
    let keyflow = Keyflow {
        who: &riley,
        org: ORG.to_owned(),
    };
    let expected = chordsheet_songs(&CHORDSHEET_LIBRARIES[0]).expect("the backup imports");
    assert_eq!(
        expected.len(),
        204,
        "the backup this chapter was written against"
    );

    // Keyflow's library page: the song list is there, and it is the
    // whole backup, in the backup's order.
    let lists = keyflow.list_songlists().await;
    let jam = lists
        .iter()
        .find(|l| l.title == ADULT_JAM)
        .unwrap_or_else(|| panic!("no Adult Jam list among {lists:?}"));
    let wanted: Vec<String> = expected.iter().map(|e| e.song_slug.clone()).collect();
    assert_eq!(ids(jam), wanted, "Adult Jam is not the backup, in order");

    // Every song is a song, and every chart a chart — the library as the
    // lanes list it, not only as the list names it.
    let songs = riley
        .resources()
        .await
        .list_songs()
        .await
        .expect("list songs");
    let charts = keyflow.list_charts("").await;
    assert!(songs.len() >= 204 && charts.len() >= 204);

    // Opening a song opens its chart, and the chart is the one from the
    // backup — byte for byte, which is what "imported" has to mean.
    for slug in [
        "mr-brightside",
        "holiday",
        "holiday-2",
        "dreams-2",
        "thunderstruck",
    ] {
        let chart = keyflow.open_song(slug).await;
        let source = &expected
            .iter()
            .find(|e| e.song_slug == slug)
            .unwrap_or_else(|| panic!("`{slug}` is not in the backup"))
            .chart
            .source;
        assert_eq!(
            &chart.source, source,
            "`{slug}` opened someone else's chart"
        );
        assert!(chart.is_default, "`{slug}`'s only chart is not its default");
    }
    // Two songs share a title across artists and stay two songs.
    let holiday = riley
        .resources()
        .await
        .song("holiday".to_owned())
        .await
        .expect("holiday");
    let holiday_2 = riley
        .resources()
        .await
        .song("holiday-2".to_owned())
        .await
        .expect("holiday-2");
    assert_ne!(
        holiday.writers, holiday_2.writers,
        "Green Day's Holiday and Weezer's are different songs"
    );
}

/// The season: setlists drawn from Adult Jam, shows made of setlists, one
/// setlist shared by two shows — and a new show put together over the wire.
///
/// t[verify project.setlist.source] — one level up: a show is assembled by
/// reference from setlists, and a setlist sits in any number of shows.
#[tokio::test]
async fn setlists_come_from_adult_jam_and_shows_run_them_in_order() {
    let s = Scenario::open().await;
    let (_server, riley) = rockstars(&s).await;
    let keyflow = Keyflow {
        who: &riley,
        org: ORG.to_owned(),
    };
    let arranged = Collections {
        who: &riley,
        org: ORG.to_owned(),
    };
    let jam = keyflow
        .list_songlists()
        .await
        .into_iter()
        .find(|l| l.title == ADULT_JAM)
        .expect("Adult Jam");
    let in_jam: std::collections::HashSet<String> = ids(&jam).into_iter().collect();

    // ── the planted season ───────────────────────────────────────────
    let setlists = arranged.list("setlist").await;
    let titles: std::collections::BTreeSet<&str> =
        setlists.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Closing Set", "Grunge Set", "Opening Set"]
            .into_iter()
            .collect()
    );
    for set in &setlists {
        for song in ids(set) {
            assert!(
                in_jam.contains(&song),
                "`{}` holds `{song}`, which is not in Adult Jam — a setlist is \
                 drawn from the library",
                set.title
            );
        }
    }

    // A show runs its setlists in order, and walking it reaches every
    // chart: show → setlist → song → the chart Keyflow opens.
    let spring = arranged.named("show", "Spring Showcase").await;
    let mut running_order = Vec::new();
    for item in &spring.items {
        let set = arranged.get(&item.node.id).await;
        running_order.push(set.title.clone());
        for song in ids(&set) {
            assert!(
                keyflow.open_song(&song).await.source.contains('|'),
                "`{song}` in `{}` opened no chart",
                set.title
            );
        }
    }
    assert_eq!(running_order, ["Opening Set", "Grunge Set", "Closing Set"]);

    // ── by reference, one level up ───────────────────────────────────
    //
    // The Opening Set is in both shows. Tightening it for the spring
    // showcase tightens it for the block party, because neither show ever
    // held a copy.
    let opening = arranged.named("setlist", "Opening Set").await;
    let first = opening.items[0].node.clone();
    let last = opening.items.last().expect("a set").node.clone();
    let moved = arranged
        .reorder(&opening.id, first.clone(), Some(last.clone()))
        .await;
    assert_eq!(
        ids(&moved).last(),
        Some(&first.id),
        "the opener did not move to the end"
    );

    let party = arranged.named("show", "Summer Block Party").await;
    let party_opening = arranged.get(&party.items[0].node.id).await;
    assert_eq!(
        ids(&party_opening),
        ids(&moved),
        "the block party is holding a stale copy of the Opening Set"
    );

    // ── a new show, put together over the wire ───────────────────────
    //
    // Pick three songs out of Adult Jam into a new set, put it in a new
    // show ahead of the planted Closing Set, then swap their order.
    let encore = arranged.create("Fall Encores", "setlist").await;
    for song in ids(&jam)
        .into_iter()
        .filter(|s| s.contains("christmas"))
        .take(3)
    {
        arranged
            .place(&encore.id, NodeRef::song(&song), None)
            .await
            .expect("pick from Adult Jam");
    }
    let closing = arranged.named("setlist", "Closing Set").await;
    let fall = arranged.create("Fall Recital", "show").await;
    arranged
        .place(&fall.id, collection(&encore.id), None)
        .await
        .expect("a setlist into a show");
    arranged
        .place(&fall.id, collection(&closing.id), None)
        .await
        .expect("a second setlist into the show");
    let swapped = arranged
        .reorder(
            &fall.id,
            collection(&encore.id),
            Some(collection(&closing.id)),
        )
        .await;
    assert_eq!(ids(&swapped), vec![closing.id.clone(), encore.id.clone()]);
    assert_eq!(
        arranged.list("show").await.len(),
        3,
        "the planted two, and this one"
    );

    // ── the one thing nesting must never do ──────────────────────────
    //
    // A show inside a setlist that the show already holds would be a
    // collection that contains itself. Refused where the edge would be
    // made, so nothing that walks a show has to defend against a loop.
    let refused = arranged
        .place(&closing.id, collection(&fall.id), None)
        .await;
    assert!(
        refused.is_err(),
        "the Closing Set was allowed to hold a show that holds the Closing Set"
    );
    let refused_self = arranged.place(&fall.id, collection(&fall.id), None).await;
    assert!(refused_self.is_err(), "a show was allowed to hold itself");
}
