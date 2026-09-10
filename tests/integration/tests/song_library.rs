#![allow(clippy::large_futures)]
//! Chapter — **one organisation subscribes to another's song library.**
//!
//! This is the sentence ADR 0004 decision 1 exists to make true, and
//! the one an earlier draft of the same ADR made impossible:
//!
//! > `<org>/assets/songs/` is a song library another organisation can
//! > subscribe to, exactly as it subscribes to a wiki, and gets the
//! > visibility rules, materialisation and reference resolution that
//! > already exist for wikis.
//!
//! `tests/integration/tests/setlist.rs` is the sibling chapter and asks
//! a different question: whether a *reference* into another org may be
//! followed, and whether dropping the subscription takes that away.
//! This one asks what happens after it may — whether the bytes actually
//! arrive on the subscriber's disk, and whether a subscriber can take
//! part of a shelf rather than all of it.
//!
//! # Why the distinction between the two is the whole design
//!
//! Under the draft that filed assets at `<vault>/Assets/Songs/`, the
//! first question had an answer and the second had none. A vault is
//! never subscribable (`wiki.boundary.no-subscribe`) — so a shelf
//! inside one was not either, resolution handed back a path under
//! `vault/` that no route served, and `docs/spec/unmet.md` carried the
//! gap as a recorded regression against what ADR 0003 had shipped.
//!
//! Nothing about collaboration required that filing. Per-file CRDT
//! follows *registration*, not directory (`org_proto::shelf`), which
//! the `wiki/` tier has demonstrated from outside the vault all along.
//! So the shelf moved out to `<org>/assets/<group>/`, kept every bit of
//! its collaboration, and gained the thing that matters here: it is a
//! shelf, and a shelf is subscribable.
//!
//! # Granularity, and why it is a facet and not a glob
//!
//! ADR 0004 decision 1a: a subscription names a shelf *or part of one*.
//! An asset group "live-tracks" holding "Worship Tracks" and "Pop
//! Tracks" should admit a subscriber who wants the worship material and
//! not the rest, and whole-shelf is the degenerate case.
//!
//! The selection is a set of **facet names** — `files.sync.selective`'s
//! own vocabulary, because a device deciding what to hold locally is
//! the same question at a smaller scope and two selection systems would
//! be one too many. A glob would have been easier and worse: a glob
//! describes a layout, so reorganising a shelf silently changes what a
//! subscriber receives, where a facet describes a class of content and
//! survives the reorganisation.

use links_proto::{NodeKind, NodeRef, Reach};
use org_proto::Selection;
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

use integration::client::Session;
use integration::scenario::Scenario;

/// The guest musician's own organisation — the example's personal org,
/// publishing under `alice.test`.
const GUEST_ORG: &str = "alice-personal";
const GUEST_DOMAIN: &str = "alice.test";

/// The asset group songs live on. Its name is the subscription slug,
/// because each group is its own shelf: there is no separate registry
/// of publishable things to keep in step with the directory listing.
const SONGS: &str = "songs";

/// A second group, shaped like the ADR's own example: one shelf holding
/// two collections a subscriber might want separately.
const LIVE_TRACKS: &str = "live-tracks";

fn song(title: &str, key: &str) -> resources_proto::SongDoc {
    resources_proto::SongDoc {
        slug: String::new(),
        title: title.into(),
        writers: vec!["Alice".into()],
        key: key.into(),
        tags: vec!["worship".into()],
        updated_at: "2026-09-10T10:00:00Z".into(),
    }
}

fn source(slug: &str, selection: Selection) -> Subscription {
    Subscription {
        domain: GUEST_DOMAIN.into(),
        slug: slug.into(),
        kind: SourceKind::Assets,
        title: slug.into(),
        core: false,
        declined: false,
        selection,
    }
}

/// Where a subscriber keeps what it took: `subscribed/<domain>/<slug>/`,
/// the same address a reference uses, so the path on disk and the name
/// in a page cannot drift apart.
fn held_copy(org_root: &std::path::Path, slug: &str) -> std::path::PathBuf {
    org_root.join("subscribed").join(GUEST_DOMAIN).join(slug)
}

async fn resolve(who: &Session, node: NodeRef) -> links_proto::ResolvedNode {
    who.links()
        .await
        .resolve_nodes(vec![node])
        .await
        .expect("resolve")
        .pop()
        .expect("one answer per node")
}

/// The studio takes the guest's whole song library, and a song from it
/// both resolves and is readable on the studio's own disk.
///
/// t[verify wiki.subscribe.local-copy] — for an asset group: after the
/// refresh the shelf is on the subscriber's disk, so it reads with the
/// network down.
/// t[verify wiki.boundary.no-subscribe] — and a vault is still refused
/// by the same code, in the chapter that widened what may be
/// subscribed to.
#[tokio::test]
async fn one_org_subscribes_to_anothers_song_library_and_resolves_a_song_from_it() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let acme_root = s.orgs.acme.org_root();

    // ── the guest org, on the same disk ──────────────────────────────
    let guest = s
        .orgs
        .acme
        .start_beside("Alice Personal", GUEST_ORG, |_| {})
        .await;
    let owner = integration::people::account(&guest, "alice@alice.test", "Alice").await;
    let guest_session = Session::open(&guest, owner.token.clone()).await;

    // ── the guest publishes two songs, through the ordinary lane ─────
    //
    // Nothing about publishing is a separate act. `upsert_song` writes
    // the document onto `<org>/assets/songs/`, and that shelf is
    // published because it exists — an org that does not want a group
    // read does not put it under `assets/`, which is stated in
    // `LocalOrgs::admits` and worth knowing here.
    for (title, key) in [("Hosanna", "D"), ("Doxology", "G")] {
        guest_session
            .resources()
            .await
            .upsert_song(song(title, key))
            .await
            .expect("the guest saves their song");
    }
    assert!(
        guest.org_root().join("assets/songs/hosanna.md").is_file(),
        "the song document is on the songs shelf, outside the vault — \
         which is what makes it subscribable at all"
    );

    // ── before subscribing: addressable, and not readable ────────────
    let hosanna = NodeRef::new(NodeKind::Song, "hosanna").in_domain(GUEST_DOMAIN);
    assert_eq!(
        resolve(&alice, hosanna.clone()).await.reach,
        Reach::NotPermitted,
        "writing the guest's domain into a reference granted a read"
    );

    // ── the subscription ─────────────────────────────────────────────
    let subs = alice.wiki_subscriptions().await;
    subs.subscribe(Subscriber::Vault, source(SONGS, Selection::All))
        .await
        .expect("a song library is a shelf, and a shelf is subscribable");

    // ── and the bytes arrive ─────────────────────────────────────────
    //
    // The step the `<vault>/Assets/` draft could not take. Resolution
    // alone was never the claim: ADR 0003 chose `resources/` precisely
    // because that tree was the one another org could *fetch*, and a
    // reference resolving to bytes nobody can read is a worse answer
    // than a refusal, because it looks like it worked.
    let report = subs
        .refresh_subscription(Subscriber::Vault, format!("{GUEST_DOMAIN}/{SONGS}"))
        .await
        .expect("refresh the song library");
    assert_eq!(report.pulled, 2, "both songs came down: {report:?}");

    let copy = held_copy(&acme_root, SONGS);
    let text = std::fs::read_to_string(copy.join("hosanna.md"))
        .expect("the song is on the studio's own disk now");
    assert!(
        text.contains("Hosanna"),
        "and it is the guest's document, not a stub: {text}"
    );

    // ── and the reference resolves, naming the publisher's own path ──
    let resolved = resolve(&alice, hosanna).await;
    assert_eq!(resolved.reach, Reach::Reachable);
    assert_eq!(resolved.org, GUEST_ORG, "the answer names the publisher");
    assert_eq!(
        resolved.rel_path, "assets/songs/hosanna.md",
        "a reachable reference says where the content sits in its own \
         org, and for a song that is its document on the songs shelf"
    );

    // ── a vault is still not subscribable, by the same code ──────────
    //
    // The rule an asset shelf must not have loosened. It is checked
    // here, in the chapter that widened what may be subscribed to,
    // because that is where a loosening would have been introduced.
    assert!(
        subs.subscribe(Subscriber::Vault, source("vault", Selection::All))
            .await
            .is_err(),
        "a vault was subscribable — sharing a note out of one goes \
         through a share link, never a subscription"
    );
}

/// A subscriber takes the worship material off a shelf and leaves the
/// rest — ADR 0004 decision 1a, at the organisation scope.
///
/// t[verify files.sync.selective] — the facet vocabulary, one scope up:
/// what a device holds of a root and what an organisation takes of a
/// shelf are the same question, and this is the second of them.
#[tokio::test]
async fn a_subscription_takes_part_of_a_shelf_and_says_what_it_left() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let acme_root = s.orgs.acme.org_root();
    let guest = s
        .orgs
        .acme
        .start_beside("Alice Personal", GUEST_ORG, |_| {})
        .await;

    // The ADR's own example, on disk: one asset group holding two
    // collections. Written directly because how the guest filled the
    // shelf is a different chapter; what is under test is what a
    // subscriber may take off it.
    let shelf = guest.org_root().join("assets").join(LIVE_TRACKS);
    for (facet, name) in [
        ("Worship Tracks", "hosanna.md"),
        ("Worship Tracks", "doxology.md"),
        ("Pop Tracks", "midnight-drive.md"),
    ] {
        let dir = shelf.join(facet);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), format!("---\ntitle: {name}\n---\n")).unwrap();
    }

    let subs = alice.wiki_subscriptions().await;
    subs.subscribe(
        Subscriber::Vault,
        source(
            LIVE_TRACKS,
            Selection::Facets(vec!["Worship Tracks".to_owned()]),
        ),
    )
    .await
    .expect("subscribe to part of the shelf");

    let report = subs
        .refresh_subscription(Subscriber::Vault, format!("{GUEST_DOMAIN}/{LIVE_TRACKS}"))
        .await
        .expect("refresh");
    assert_eq!(
        report.pulled, 2,
        "the two worship tracks came down and nothing else: {report:?}"
    );

    let copy = held_copy(&acme_root, LIVE_TRACKS);
    assert!(copy.join("Worship Tracks/hosanna.md").is_file());
    assert!(copy.join("Worship Tracks/doxology.md").is_file());
    assert!(
        !copy.join("Pop Tracks/midnight-drive.md").exists(),
        "the pop material arrived anyway — a selection that fetches \
         everything is not a selection"
    );

    // Whole-shelf is the degenerate case, and taking it changes only
    // the selection. Re-subscribing after dropping it is how a person
    // widens what they take; nothing else about the subscription moves.
    subs.unsubscribe(
        Subscriber::Vault,
        format!("{GUEST_DOMAIN}/{LIVE_TRACKS}"),
        true,
    )
    .await
    .expect("drop it");
    subs.subscribe(Subscriber::Vault, source(LIVE_TRACKS, Selection::All))
        .await
        .expect("and take the whole shelf instead");
    let whole = subs
        .refresh_subscription(Subscriber::Vault, format!("{GUEST_DOMAIN}/{LIVE_TRACKS}"))
        .await
        .expect("refresh");
    assert_eq!(
        whole.pulled, 1,
        "only the file the narrower selection had left behind: {whole:?}"
    );
    assert!(
        copy.join("Pop Tracks/midnight-drive.md").is_file(),
        "widening the selection brought the rest of the shelf"
    );
}
