#![allow(clippy::large_futures)]
//! Chapter — **three apps, one account, two servers.**
//!
//! ADR 0003's opening sentence, driven end to end: *a library of typed
//! things, referenced into a performance, shared across organisations* —
//! where the organisations are now on different machines.
//!
//! Each app gets the half of the job it actually does:
//!
//! - **Keyflow** saves a chart, and later opens the chart for a song in
//!   somebody else's setlist.
//! - **Signal** saves a patch, so the rig for a song travels with the
//!   song rather than beside it.
//! - **Session** builds the setlist, referencing a song it does not own
//!   and never copies.
//!
//! Nothing here is app-specific on the server. Every call is one an
//! ordinary member makes with an ordinary token, through the permit
//! table — which is decision 4 ("the apps are clients, not backends")
//! being true rather than asserted. The chapter would read the same if a
//! fourth app arrived tomorrow.
//!
//! # Why two servers is the point
//!
//! The one-disk version of this already passes (`charts.rs`,
//! `song_library.rs`, `setlist.rs`) and proves less than it looks: two
//! orgs on one data root share a filesystem, so "the studio reaches the
//! guest's library" can be true because the directory is simply there.
//! A guest musician has their own server. So the library here is taken
//! by subscription across a boundary, and what the apps read is the
//! copy that brought back.
//!
//! # The two tiers, and the reason both had to cross
//!
//! ADR 0004 put charts and songs on the **Assets** tier, where people
//! type. ADR 0003 left patches and samples on **Resources**, as
//! manifests small enough for a subscription to carry, with the bytes
//! they name in a File Root. Signal needs the second, so a remote
//! Resource had to stop being refused — it was, briefly, on the grounds
//! that an edition is installed rather than pulled, which is true of
//! scripture and of nothing else on that tier.

use collection_proto::{CollectionKind, Placement};
use integration::client::Session;
use integration::scenario::Scenario;
use links_proto::{NodeKind, NodeRef, Reach};
use resources_proto::{ChartDoc, PatchDoc, SongDoc};
use wiki_proto::service::subscriptions::SourceGrant;
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

/// The guest's domain — VNT's server, which is not ACME's disk.
const GUEST: &str = "vnt.test";
const SONGS: &str = "songs";
const CHARTS: &str = "charts";
const PATCHES: &str = "patches";

fn song(title: &str, key: &str) -> SongDoc {
    SongDoc {
        slug: String::new(),
        title: title.into(),
        writers: vec!["Victor".into()],
        key: key.into(),
        tags: vec!["set".into()],
        updated_at: "2026-09-21T09:00:00Z".into(),
    }
}

fn chart(title: &str, song_slug: &str, body: &str) -> ChartDoc {
    ChartDoc {
        slug: String::new(),
        title: title.into(),
        source: body.into(),
        song: song_slug.into(),
        ..Default::default()
    }
}

fn patch(title: &str, rig: &str) -> PatchDoc {
    PatchDoc {
        slug: String::new(),
        title: title.into(),
        rig: rig.into(),
        tags: vec!["lead".into()],
        body: "gain 6\nreverb 22\n".into(),
        updated_at: "2026-09-21T09:00:00Z".into(),
        ..Default::default()
    }
}

/// Hand one of the guest's libraries to the studio, the way a person
/// does it: the publisher grants, the subscriber records, subscribes and
/// refreshes.
///
/// Four calls, all ordinary. The secret travels between them the way it
/// travels in life — somebody carries it.
async fn take_library(
    s: &Scenario,
    studio: &Session,
    guest: &Session,
    kind: SourceKind,
    slug: &str,
) {
    let secret = guest
        .wiki_subscriptions()
        .await
        .grant_source_read(kind, slug.to_owned())
        .await
        .unwrap_or_else(|e| panic!("the guest grants read on `{slug}`: {e:?}"));
    let subs = studio.wiki_subscriptions().await;
    subs.trust_source(SourceGrant {
        domain: GUEST.to_owned(),
        endpoint: s.orgs.vnt.endpoint.id().to_string(),
        kind,
        slug: slug.to_owned(),
        secret,
    })
    .await
    .expect("the studio records the grant");
    subs.subscribe(
        Subscriber::Vault,
        Subscription {
            domain: GUEST.into(),
            slug: slug.into(),
            kind,
            title: slug.into(),
            core: false,
            declined: false,
            selection: Default::default(),
        },
    )
    .await
    .unwrap_or_else(|e| panic!("subscribe to `{slug}`: {e:?}"));
    let report = subs
        .refresh_subscription(Subscriber::Vault, format!("{GUEST}/{slug}"))
        .await
        .unwrap_or_else(|e| panic!("refresh `{slug}`: {e:?}"));
    assert!(
        report.pulled > 0,
        "`{slug}` came down empty: {report:?} — a library that arrives \
         with nothing in it looks exactly like one that worked"
    );
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

/// The whole journey, in the order the three apps take it.
///
/// t[verify wiki.subscribe.federated] — for both tiers at once, which is
/// what an app actually needs: a song and its chart from the Assets
/// tier, its patch from Resources.
/// t[verify wiki.subscribe.resolution] — a setlist reaches a song, a
/// chart and a rig that live on another company's server.
#[tokio::test(flavor = "multi_thread")]
async fn a_setlist_reaches_a_song_its_chart_and_its_rig_on_another_server() {
    let s = Scenario::open().await;
    let studio = s.as_alice().await;
    let guest = s.as_victor().await;

    // ── the guest's own libraries, written by the apps that own them ──
    //
    // Session saves the song, Keyflow the chart, Signal the patch. Three
    // apps, three calls, one account — and on the server they are three
    // upserts through one lane.
    let saved_song = guest
        .resources()
        .await
        .upsert_song(song("Reel Theme", "A"))
        .await
        .expect("Session saves the guest's song");
    let saved_chart = guest
        .resources()
        .await
        .upsert_chart(chart(
            "Reel Theme",
            &saved_song.slug,
            "[Verse]\n| Am | F | C | G |\n",
        ))
        .await
        .expect("Keyflow saves the chart for it");
    let saved_patch = guest
        .resources()
        .await
        .upsert_patch(patch("Reel Lead", "helix"))
        .await
        .expect("Signal saves the rig for it");
    assert_eq!(saved_song.slug, "reel-theme");
    assert_eq!(saved_chart.slug, "reel-theme");

    // ── before the subscriptions: addressable, and not readable ──────
    //
    // The studio can *write* the guest's domain into a reference. That
    // grants nothing, which is the security claim of the qualified form
    // and the reason this assertion comes first.
    //
    // `NotPermitted` rather than `UnknownDomain` because the seed gives
    // every example org an `<name>.test` name on every server, so ACME
    // recognises the *name* `vnt.test` while holding nothing published
    // under it. Both are refusals; they differ only in what the
    // deployment admits knowing, and neither tells an outsider which
    // slugs exist.
    for (kind, id) in [
        (NodeKind::Song, saved_song.slug.as_str()),
        (NodeKind::Chart, saved_chart.slug.as_str()),
        (NodeKind::Patch, saved_patch.slug.as_str()),
    ] {
        let answer = resolve(&studio, NodeRef::new(kind, id).in_domain(GUEST)).await;
        assert!(
            !answer.is_reachable(),
            "{kind:?}:{id} resolved on no subscription: {answer:?}"
        );
        assert_eq!(answer.reach, Reach::NotPermitted, "{kind:?}:{id}");
    }

    // ── the guest hands over three libraries ─────────────────────────
    take_library(&s, &studio, &guest, SourceKind::Assets, SONGS).await;
    take_library(&s, &studio, &guest, SourceKind::Assets, CHARTS).await;
    take_library(&s, &studio, &guest, SourceKind::Resource, PATCHES).await;

    // ── Session builds the setlist, by reference ─────────────────────
    //
    // `project.setlist.source`: a setlist references songs, it does not
    // copy them. The reference carries the guest's domain, so this
    // setlist names a song that has never been in ACME's library.
    let collections = studio.collections().await;
    let setlist = collections
        .create(
            s.orgs.acme.slug.clone(),
            "Sunday".to_owned(),
            CollectionKind::new("setlist"),
        )
        .await
        .expect("Session creates the setlist");
    let setlist = collections
        .add_item(Placement {
            collection_id: setlist.id.clone(),
            node: NodeRef::new(NodeKind::Song, &saved_song.slug).in_domain(GUEST),
            after: None,
        })
        .await
        .expect("and puts the guest's song in it");

    assert_eq!(setlist.items.len(), 1);
    assert_eq!(
        setlist.items[0].node.domain, GUEST,
        "the setlist holds a reference to the guest's org, not a copy \
         of their song"
    );

    // ── and every app reaches what it needs through that reference ───
    //
    // Session the song, Keyflow the chart, Signal the rig — each one
    // resolving to a file that is really on the studio's disk, because a
    // subscription brought it there.
    for (kind, id, expected) in [
        (
            NodeKind::Song,
            saved_song.slug.as_str(),
            format!("subscribed/{GUEST}/{SONGS}/{}.md", saved_song.slug),
        ),
        (
            NodeKind::Chart,
            saved_chart.slug.as_str(),
            format!("subscribed/{GUEST}/{CHARTS}/{}.md", saved_chart.slug),
        ),
        (
            NodeKind::Patch,
            saved_patch.slug.as_str(),
            format!("subscribed/{GUEST}/{PATCHES}/{}", saved_patch.slug),
        ),
    ] {
        let answer = resolve(&studio, NodeRef::new(kind, id).in_domain(GUEST)).await;
        assert_eq!(
            answer.reach,
            Reach::Reachable,
            "{kind:?}:{id} did not reach: {answer:?}"
        );
        assert_eq!(answer.rel_path, expected, "{kind:?}:{id}");
        assert!(
            s.orgs.acme.org_root().join(&answer.rel_path).exists(),
            "{kind:?}:{id} named a path with nothing at it"
        );
    }

    // The chart really is the guest's chart, not a stub with the right
    // name — the difference between a reference that resolves and one
    // that works.
    let chart_copy = s.orgs.acme.org_root().join(format!(
        "subscribed/{GUEST}/{CHARTS}/{}.md",
        saved_chart.slug
    ));
    let text = std::fs::read_to_string(&chart_copy).expect("the chart on the studio's disk");
    assert!(
        text.contains("| Am | F | C | G |"),
        "Keyflow would open an empty chart: {text}"
    );
}

/// The guest takes their library back, and the setlist stops reaching.
///
/// The half that makes the other half mean something. A setlist that
/// keeps working after the subscription ends would mean the subscription
/// was never what authorised it — and the copy is still on the studio's
/// disk throughout, so what is being tested is the rule and not the
/// filesystem.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_the_library_takes_the_setlists_reach_away() {
    let s = Scenario::open().await;
    let studio = s.as_alice().await;
    let guest = s.as_victor().await;

    let saved = guest
        .resources()
        .await
        .upsert_song(song("Reel Theme", "A"))
        .await
        .expect("the guest's song");
    take_library(&s, &studio, &guest, SourceKind::Assets, SONGS).await;

    let node = NodeRef::new(NodeKind::Song, &saved.slug).in_domain(GUEST);
    assert_eq!(resolve(&studio, node.clone()).await.reach, Reach::Reachable);

    studio
        .wiki_subscriptions()
        .await
        .unsubscribe(Subscriber::Vault, format!("{GUEST}/{SONGS}"), true)
        .await
        .expect("the studio drops the library");

    let after = resolve(&studio, node).await;
    assert_eq!(
        after.reach,
        Reach::NotPermitted,
        "the setlist still reached the guest's song with no subscription \
         behind it: {after:?}"
    );
}
