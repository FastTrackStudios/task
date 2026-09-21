#![allow(clippy::large_futures)]
//! Chapter — **Keyflow keeps a working library here**: charts saved as a
//! person writes them, gathered into song lists, and setlists drawn from
//! those lists for a service.
//!
//! `charts.rs` is the first claim ADR 0003 made for Keyflow: an outside
//! app saves a chart with ordinary RPCs and nothing is built for it. This
//! chapter is what a worship leader actually does with that library over
//! a week, end to end, and it is the contract Keyflow's shipped code
//! depends on.
//!
//! # Keyflow's calls, not a paraphrase of them
//!
//! [`Keyflow`] below is a line-for-line mirror of keyflow's
//! `apps/web/src/library/vox.rs`: the same RPCs, with the same arguments
//! in the same shapes — a song created with an empty slug so the server
//! derives it, a chart that names its song as a `song:<slug>` token, a
//! song list that is a collection of kind `"songlist"` (`SONGLIST_KIND`
//! there), an item appended with `after: None`. And [`Keyflow::open_song`]
//! is keyflow's `default_chart` rule: the chart the server marks default,
//! else the first. If Task changes any of that, this chapter fails here
//! before Keyflow fails in a browser.
//!
//! Keyflow has no setlists of its own yet — that is Session's half of the
//! job — so [`Setlists`] is the same primitive under the kind the seed
//! already uses (`"setlist"`): a collection whose items are the *same*
//! references the song lists hold.
//!
//! # By reference is the claim, so it is tested by change
//!
//! A setlist that happened to hold copies would pass every read-back test
//! and fail the first time somebody fixed a chord. So the chapter edits a
//! chart after the setlist is built and opens it again *through the
//! setlist*; takes a song out of the list it was drawn from and checks the
//! setlist still has it; and restarts the server to check that what a
//! person arranged is what comes back, in the order they arranged it.

use collection_proto::{Collection, CollectionKind, Placement};
use integration::client::Session;
use integration::scenario::Scenario;
use links_proto::NodeRef;
use resources_proto::{ChartDoc, ChartSummary, SongDoc};

/// Keyflow's `SONGLIST_KIND`.
const SONGLIST: &str = "songlist";
/// What the seed, and Session, call a setlist.
const SETLIST: &str = "setlist";

/// The chart to open for a song that names no arrangement: the one the
/// server marks default, else the first. Keyflow's `default_chart`,
/// verbatim — the fallback is there for imported libraries, where a
/// song's only chart may carry no flag.
fn default_chart(charts: &[ChartSummary]) -> Option<&ChartSummary> {
    charts
        .iter()
        .find(|chart| chart.is_default)
        .or_else(|| charts.first())
}

/// Keyflow, as a client of Task. One method per call keyflow's library
/// makes; nothing here is a convenience Keyflow does not have.
struct Keyflow<'a> {
    who: &'a Session,
    org: String,
}

impl Keyflow<'_> {
    /// `create_song`: an empty slug so the server derives it, and the
    /// title and key trimmed the way the editor's fields are.
    async fn create_song(&self, title: &str, key: &str) -> String {
        self.who
            .resources()
            .await
            .upsert_song(SongDoc {
                slug: String::new(),
                title: title.trim().to_owned(),
                writers: Vec::new(),
                key: key.trim().to_owned(),
                tags: Vec::new(),
                updated_at: String::new(),
            })
            .await
            .unwrap_or_else(|e| panic!("Keyflow creates `{title}`: {e:?}"))
            .slug
    }

    /// `save_chart`: the chart names its song as a `song:<slug>` token.
    /// `slug` empty for a new chart; the stored slug to save an edit.
    async fn save_chart(&self, chart: Chart<'_>) -> String {
        self.who
            .resources()
            .await
            .upsert_chart(ChartDoc {
                slug: chart.slug.to_owned(),
                title: chart.title.to_owned(),
                source: chart.source.to_owned(),
                key: chart.key.to_owned(),
                song: NodeRef::song(chart.song).to_token(),
                arrangement: chart.arrangement.to_owned(),
                is_default: chart.make_default,
                updated_at: "2026-09-21T09:00:00Z".to_owned(),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("Keyflow saves `{}`: {e:?}", chart.title))
            .slug
    }

    async fn list_charts(&self, song: &str) -> Vec<ChartSummary> {
        self.who
            .resources()
            .await
            .list_charts(song.to_owned())
            .await
            .expect("list charts")
    }

    /// What tapping a song does: its charts, the default one, its source.
    async fn open_song(&self, song: &str) -> ChartDoc {
        let charts = self.list_charts(song).await;
        let chart = default_chart(&charts)
            .unwrap_or_else(|| panic!("`{song}` has no chart to open"))
            .slug
            .clone();
        self.who
            .resources()
            .await
            .chart(chart.clone())
            .await
            .unwrap_or_else(|e| panic!("open `{chart}`: {e:?}"))
    }

    async fn list_songlists(&self) -> Vec<Collection> {
        self.who
            .collections()
            .await
            .list(self.org.clone(), Some(CollectionKind::new(SONGLIST)))
            .await
            .expect("list song lists")
    }

    async fn create_songlist(&self, title: &str) -> Collection {
        self.who
            .collections()
            .await
            .create(
                self.org.clone(),
                title.trim().to_owned(),
                CollectionKind::new(SONGLIST),
            )
            .await
            .unwrap_or_else(|e| panic!("create `{title}`: {e:?}"))
    }

    async fn add_to_songlist(&self, list: &str, song: &str) -> Collection {
        self.who
            .collections()
            .await
            .add_item(Placement {
                collection_id: list.to_owned(),
                node: NodeRef::song(song),
                after: None,
            })
            .await
            .unwrap_or_else(|e| panic!("add `{song}` to a list: {e:?}"))
    }

    async fn remove_from_songlist(&self, list: &str, song: &str) -> Collection {
        self.who
            .collections()
            .await
            .remove_item(list.to_owned(), NodeRef::song(song))
            .await
            .unwrap_or_else(|e| panic!("remove `{song}` from a list: {e:?}"))
    }
}

/// One chart as the editor saves it.
struct Chart<'a> {
    slug: &'a str,
    title: &'a str,
    song: &'a str,
    arrangement: &'a str,
    key: &'a str,
    source: &'a str,
    make_default: bool,
}

/// Setlists — the same primitive under the seed's kind, holding the same
/// references the song lists do.
struct Setlists<'a> {
    who: &'a Session,
    org: String,
}

impl Setlists<'_> {
    async fn create(&self, title: &str) -> Collection {
        self.who
            .collections()
            .await
            .create(
                self.org.clone(),
                title.to_owned(),
                CollectionKind::new(SETLIST),
            )
            .await
            .unwrap_or_else(|e| panic!("create setlist `{title}`: {e:?}"))
    }

    /// Put `node` in the setlist straight after `after`, or at the end.
    async fn place(&self, setlist: &str, node: NodeRef, after: Option<NodeRef>) -> Collection {
        self.who
            .collections()
            .await
            .add_item(Placement {
                collection_id: setlist.to_owned(),
                node,
                after,
            })
            .await
            .expect("place a song in the setlist")
    }

    async fn reorder(&self, setlist: &str, node: NodeRef, after: Option<NodeRef>) -> Collection {
        self.who
            .collections()
            .await
            .reorder(Placement {
                collection_id: setlist.to_owned(),
                node,
                after,
            })
            .await
            .expect("reorder the setlist")
    }
}

async fn get(who: &Session, id: &str) -> Collection {
    who.collections()
        .await
        .get(id.to_owned())
        .await
        .expect("get")
        .unwrap_or_else(|| panic!("collection `{id}` is gone"))
}

/// The song slugs a collection holds, in its order.
fn songs(c: &Collection) -> Vec<String> {
    c.items.iter().map(|i| i.node.id.clone()).collect()
}

/// A small, real library: six songs across two traditions, one of them
/// with a second arrangement.
const LIBRARY: &[(&str, &str, &str)] = &[
    (
        "Great Is Thy Faithfulness",
        "D",
        "[Verse]\n| D | G | D | A |\n",
    ),
    (
        "Be Thou My Vision",
        "Eb",
        "[Verse]\n| Eb | Ab | Eb | Bb |\n",
    ),
    ("Come Thou Fount", "D", "[Verse]\n| D | A | Bm | G |\n"),
    ("Build My Life", "G", "[Verse]\n| G | C | Em | D |\n"),
    ("Goodness of God", "A", "[Verse]\n| A | D | A | E |\n"),
    ("Way Maker", "E", "[Verse]\n| E | B | C#m | A |\n"),
];

/// The whole week, in the order a person does it.
///
/// t[verify project.setlist.source] — a setlist is assembled by reference
/// from songs other collections already hold, and a song sits in any
/// number of lists and setlists at once.
#[tokio::test]
async fn keyflow_stores_charts_organises_them_into_lists_and_setlists_draw_on_them() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let org = s.orgs.acme.slug.clone();
    let keyflow = Keyflow {
        who: &alice,
        org: org.clone(),
    };
    let setlists = Setlists {
        who: &alice,
        org: org.clone(),
    };

    // ── 1. Keyflow stores charts, the way its editor saves them ──────
    //
    // Save from a plain editor: create the song, then its chart. The
    // first chart of a song becomes its default whatever it asked for,
    // which is why none of these asks.
    let mut slug = std::collections::HashMap::new();
    for (title, key, source) in LIBRARY {
        let song = keyflow.create_song(title, key).await;
        keyflow
            .save_chart(Chart {
                slug: "",
                title,
                song: &song,
                arrangement: "",
                key,
                source,
                make_default: false,
            })
            .await;
        slug.insert(*title, song);
    }
    let fount = slug["Come Thou Fount"].clone();
    assert_eq!(fount, "come-thou-fount", "the server derives the slug");

    // A second arrangement of one song. Saving it does not take the
    // song's default — `is_default: false` is no opinion, and a person
    // fixing a typo in an alternate must not demote the main chart.
    let acoustic = keyflow
        .save_chart(Chart {
            slug: "",
            title: "Come Thou Fount",
            song: &fount,
            arrangement: "acoustic in C",
            key: "C",
            source: "[Verse]\n| C | G | Am | F |\n",
            make_default: false,
        })
        .await;
    assert_ne!(acoustic, fount, "an arrangement gets a slug of its own");
    assert_eq!(keyflow.list_charts(&fount).await.len(), 2);
    assert!(
        keyflow.open_song(&fount).await.source.contains("| D | A |"),
        "saving an alternate took the song's default"
    );

    // ── 2. Organised into song lists ─────────────────────────────────
    //
    // A song belongs to as many lists as describe it: Come Thou Fount is
    // a hymn *and* an opener.
    let hymns = keyflow.create_songlist("Hymns").await;
    let modern = keyflow.create_songlist("Contemporary").await;
    let openers = keyflow.create_songlist("  Openers  ").await;
    assert_eq!(
        openers.title, "Openers",
        "the title is trimmed as Keyflow sends it"
    );
    for title in [
        "Great Is Thy Faithfulness",
        "Be Thou My Vision",
        "Come Thou Fount",
    ] {
        keyflow.add_to_songlist(&hymns.id, &slug[title]).await;
    }
    for title in ["Build My Life", "Goodness of God", "Way Maker"] {
        keyflow.add_to_songlist(&modern.id, &slug[title]).await;
    }
    for title in ["Come Thou Fount", "Way Maker"] {
        keyflow.add_to_songlist(&openers.id, &slug[title]).await;
    }

    // What Keyflow's library page shows: every song list, and only song
    // lists — the seed's own "Sunday Songs" among them, and neither its
    // setlist nor its library nor its rehearsal pool.
    let lists = keyflow.list_songlists().await;
    let titles: std::collections::BTreeSet<&str> = lists.iter().map(|l| l.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Contemporary", "Hymns", "Openers", "Sunday Songs"]
            .into_iter()
            .collect(),
        "the song lists page is filtered by kind, not by what happens to be there"
    );
    // In the order they were added — lexorank, appended.
    assert_eq!(
        songs(&get(&alice, &hymns.id).await),
        vec![
            "great-is-thy-faithfulness",
            "be-thou-my-vision",
            "come-thou-fount"
        ]
    );

    // ── 3. Setlists, drawn from the lists ────────────────────────────
    //
    // Sunday morning: an opener, two hymns, a contemporary closer — each
    // pulled out of the list it lives in, which is how a person builds
    // one: open a list, pick from it.
    let morning = setlists.create("Sunday Morning").await;
    let opener = get(&alice, &openers.id).await.items[0].node.clone();
    let hymn_items = get(&alice, &hymns.id).await.items;
    let closer = get(&alice, &modern.id).await.items[1].node.clone();
    setlists.place(&morning.id, opener.clone(), None).await;
    setlists
        .place(
            &morning.id,
            hymn_items[0].node.clone(),
            Some(opener.clone()),
        )
        .await;
    setlists
        .place(
            &morning.id,
            hymn_items[1].node.clone(),
            Some(hymn_items[0].node.clone()),
        )
        .await;
    let built = setlists.place(&morning.id, closer.clone(), None).await;
    assert_eq!(
        songs(&built),
        vec![
            "come-thou-fount",
            "great-is-thy-faithfulness",
            "be-thou-my-vision",
            "goodness-of-god"
        ]
    );
    assert_eq!(
        built.items[0].node, opener,
        "the setlist holds the very reference the list holds — not a copy"
    );

    // The closer moves up: the band wants to end on a hymn.
    let reordered = setlists
        .reorder(&morning.id, closer.clone(), Some(opener.clone()))
        .await;
    assert_eq!(
        songs(&reordered),
        vec![
            "come-thou-fount",
            "goodness-of-god",
            "great-is-thy-faithfulness",
            "be-thou-my-vision"
        ],
        "only the moved song changed place"
    );

    // Sunday evening reuses a song: one song, two setlists, one chart.
    let evening = setlists.create("Sunday Evening").await;
    setlists
        .place(&evening.id, NodeRef::song(&slug["Way Maker"]), None)
        .await;
    setlists.place(&evening.id, opener.clone(), None).await;

    let all_setlists = alice
        .collections()
        .await
        .list(org.clone(), Some(CollectionKind::new(SETLIST)))
        .await
        .expect("list setlists");
    let set_titles: std::collections::BTreeSet<&str> =
        all_setlists.iter().map(|c| c.title.as_str()).collect();
    assert!(set_titles.contains("Sunday Morning") && set_titles.contains("Sunday Evening"));
    assert!(
        !set_titles.contains("Hymns"),
        "a song list leaked into the setlists"
    );

    // ── 4. Opening the service, the way Keyflow opens a song ─────────
    //
    // Every entry opens its default chart, and Come Thou Fount opens the
    // original — not the acoustic arrangement it also has.
    for item in &get(&alice, &morning.id).await.items {
        let chart = keyflow.open_song(&item.node.id).await;
        assert!(
            chart.source.starts_with("[Verse]"),
            "`{}` opened something that is not its chart",
            item.node.id
        );
    }
    assert!(keyflow.open_song(&fount).await.source.contains("| D | A |"));

    // ── 5. By reference, proven by change ────────────────────────────
    //
    // The band decides to play the acoustic arrangement from now on.
    // One explicit request moves the default; the setlist, which never
    // held a chart at all, opens the new one.
    keyflow
        .save_chart(Chart {
            slug: &acoustic,
            title: "Come Thou Fount",
            song: &fount,
            arrangement: "acoustic in C",
            key: "C",
            source: "[Verse]\n| C | G | Am | F |\n",
            make_default: true,
        })
        .await;
    let opened = keyflow
        .open_song(&get(&alice, &morning.id).await.items[0].node.id)
        .await;
    assert_eq!(
        opened.slug, acoustic,
        "the setlist still opened the old default — it was holding a chart, not a song"
    );

    // A chord is fixed in the evening's song. Both setlists see it,
    // because neither holds a copy.
    let way_maker = slug["Way Maker"].clone();
    keyflow
        .save_chart(Chart {
            slug: &way_maker,
            title: "Way Maker",
            song: &way_maker,
            arrangement: "",
            key: "E",
            source: "[Verse]\n| E | B | C#m7 | A |\n",
            make_default: false,
        })
        .await;
    assert!(
        keyflow.open_song(&way_maker).await.source.contains("C#m7"),
        "the fix did not reach the chart"
    );
    assert!(
        keyflow.open_song(&way_maker).await.is_default,
        "fixing a chord demoted the song's main chart"
    );

    // Taking a song out of a list leaves every setlist that drew on it
    // alone: the setlist chose the song, not its membership of the list.
    let hymns_after = keyflow.remove_from_songlist(&hymns.id, &fount).await;
    assert!(!songs(&hymns_after).contains(&fount));
    assert!(
        songs(&get(&alice, &morning.id).await).contains(&fount),
        "removing a song from a list removed it from a setlist"
    );
    assert!(
        songs(&get(&alice, &openers.id).await).contains(&fount),
        "removing a song from one list removed it from another"
    );

    // ── 6. The other people in the org see the same library ──────────
    //
    // Song lists are the org's, not Alice's: Sam, an employee, opens
    // Keyflow and finds them, and opens a song from one.
    let sam = Session::open(&s.orgs.acme, s.people.sam.token.clone()).await;
    let sams_keyflow = Keyflow {
        who: &sam,
        org: org.clone(),
    };
    let seen: std::collections::BTreeSet<String> = sams_keyflow
        .list_songlists()
        .await
        .into_iter()
        .map(|l| l.title)
        .collect();
    assert!(seen.contains("Hymns") && seen.contains("Openers"));
    assert!(
        sams_keyflow
            .open_song(&slug["Be Thou My Vision"])
            .await
            .source
            .contains("| Eb |")
    );
}

/// What a person arranged is what comes back after a restart, in the
/// order they arranged it — the reason Keyflow moved its library into
/// Task at all ("charts that survive the browser tab").
#[tokio::test]
async fn a_library_and_its_setlists_come_back_in_order_after_a_restart() {
    let s = Scenario::open().await;
    let org = s.orgs.acme.slug.clone();
    let (list_id, set_id, expected_list, expected_set) = {
        let alice = s.as_alice().await;
        let keyflow = Keyflow {
            who: &alice,
            org: org.clone(),
        };
        let setlists = Setlists {
            who: &alice,
            org: org.clone(),
        };
        let mut songs_in_order = Vec::new();
        for (title, key, source) in &LIBRARY[..4] {
            let song = keyflow.create_song(title, key).await;
            keyflow
                .save_chart(Chart {
                    slug: "",
                    title,
                    song: &song,
                    arrangement: "",
                    key,
                    source,
                    make_default: false,
                })
                .await;
            songs_in_order.push(song);
        }
        let list = keyflow.create_songlist("Everything").await;
        for song in &songs_in_order {
            keyflow.add_to_songlist(&list.id, song).await;
        }
        let set = setlists.create("Rehearsal").await;
        // Deliberately not the list's order, so "came back sorted by
        // something else" cannot pass by accident.
        for song in songs_in_order.iter().rev() {
            setlists.place(&set.id, NodeRef::song(song), None).await;
        }
        let set = setlists
            .reorder(&set.id, NodeRef::song(&songs_in_order[1]), None)
            .await;
        (list.id, set.id.clone(), songs_in_order.clone(), songs(&set))
    };

    let Scenario { orgs, people, .. } = s;
    let acme = orgs.acme.restart().await;
    let alice = Session::open(&acme, people.alice.token.clone()).await;

    assert_eq!(
        songs(&get(&alice, &list_id).await),
        expected_list,
        "the song list came back in a different order"
    );
    assert_eq!(
        songs(&get(&alice, &set_id).await),
        expected_set,
        "the setlist came back in a different order"
    );
    let keyflow = Keyflow {
        who: &alice,
        org: acme.slug.clone(),
    };
    for song in &expected_list {
        assert!(
            keyflow.open_song(song).await.source.starts_with("[Verse]"),
            "`{song}` has no chart after the restart"
        );
    }
}
