//! Chapter — a setlist that draws on somebody else's library.
//!
//! The sentence ADR 0003 exists for: *a library of typed things,
//! referenced into a performance, shared across organisations.* A
//! guest musician keeps their own charts in their own org; the studio
//! builds a setlist holding one of its own charts beside one of
//! theirs; and whether that second reference may be followed is decided
//! by a subscription the studio took on — never by the reference
//! itself.
//!
//! That last clause is the security claim of the decision, and it is
//! only a claim until something watches it *change*. So this chapter
//! resolves the same setlist three times — before the subscription,
//! after it, and after dropping it — and asserts the middle one is the
//! only one that reaches. A test that subscribed first and resolved
//! once could not tell resolution from a permanently open door.
//!
//! # Two orgs on one disk, and why it is that arrangement
//!
//! `LocalHomes` — the only [`links::NodeHomes`] that exists — answers
//! for orgs on the reader's own data root, which is what `admin seed`
//! and the demo produce. A qualified reference to an org on *another
//! server* parses and does not resolve, and the ADR records that as the
//! next piece of work rather than a caveat. Both halves are asserted
//! here: the guest org boots beside ACME on ACME's disk
//! ([`integration::server::Server::start_beside`]) and resolves; VNT is
//! a company on a different machine, holds a real chart, and stays
//! unreachable — including to a subscription, which is refused rather
//! than left to fail later.

use collection_proto::{CollectionKind, Placement};
use integration::client::Session;
use integration::scenario::Scenario;
use links_proto::{NodeKind, NodeRef, Reach};
use resources_proto::ChartDoc;
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

/// The guest musician's own organisation: the example's personal org,
/// which publishes under `alice.test`.
const GUEST_ORG: &str = "alice-personal";
const GUEST_DOMAIN: &str = "alice.test";
/// The subscription slug that publishes charts — fixed by
/// `node_homes::library_of(NodeKind::Chart)`, and the same string a
/// reader names when subscribing.
const CHART_LIBRARY: &str = "charts";

fn chart(title: &str, source: &str) -> ChartDoc {
    ChartDoc {
        slug: String::new(),
        title: title.into(),
        source: source.into(),
        key: "A".into(),
        notation: "keyflow".into(),
        sections: vec!["chorus".to_owned()],
        song: String::new(),
        arrangement: String::new(),
        is_default: false,
        updated_at: "2026-09-06T10:00:00Z".into(),
    }
}

/// Ask the links lane what the reader may follow — the one call a
/// setlist makes before it can draw itself.
async fn resolve(who: &Session, nodes: &[NodeRef]) -> Vec<links_proto::ResolvedNode> {
    who.links()
        .await
        .resolve_nodes(nodes.to_vec())
        .await
        .expect("resolve the setlist")
}

fn source_of(domain: &str) -> Subscription {
    Subscription {
        domain: domain.into(),
        slug: CHART_LIBRARY.into(),
        kind: SourceKind::Resource,
        title: "Charts".into(),
        core: false,
        declined: false,
    }
}

/// A setlist in ACME holding one local chart and one of the guest's,
/// resolved before, during and after the subscription that admits it.
#[tokio::test]
async fn a_setlist_reaches_another_orgs_chart_only_while_subscribed() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let org = s.orgs.acme.slug.clone();

    // ── the guest musician's own org, on the same disk ───────────────
    let guest = s
        .orgs
        .acme
        .start_beside("Alice Personal", GUEST_ORG, |_| {})
        .await;
    let owner = integration::people::account(&guest, "alice@alice.test", "Alice").await;
    let guest_session = Session::open(&guest, owner.token.clone()).await;

    // They publish a chart, through the same lane Keyflow uses.
    let published = guest_session
        .resources()
        .await
        .upsert_chart(chart("Hosanna", "[Chorus]\n| D | A | E |\n"))
        .await
        .expect("the guest saves their chart");
    assert_eq!(published.slug, "hosanna");

    // ── ACME's own chart ─────────────────────────────────────────────
    alice
        .resources()
        .await
        .upsert_chart(chart("Doxology", "[Verse]\n| G | C | D | G |\n"))
        .await
        .expect("ACME saves its own chart");

    // ── the setlist: one local reference, one qualified ──────────────
    let mine = NodeRef::new(NodeKind::Chart, "doxology");
    let theirs = NodeRef::new(NodeKind::Chart, "hosanna").in_domain(GUEST_DOMAIN);

    let setlist = alice
        .collections()
        .await
        .create(org, "Album Launch Set".into(), CollectionKind::Setlist)
        .await
        .expect("create the setlist");
    for node in [mine.clone(), theirs.clone()] {
        alice
            .collections()
            .await
            .add_item(Placement {
                collection_id: setlist.id.clone(),
                node,
                after: None,
            })
            .await
            .expect("add to the setlist");
    }
    let held = alice
        .collections()
        .await
        .get(setlist.id.clone())
        .await
        .expect("read the setlist")
        .expect("it is there");
    assert_eq!(
        held.items.len(),
        2,
        "a collection refused a reference naming another org"
    );

    // Resolving the setlist is one call for the whole of it — a
    // setlist resolves twenty items at once, and the answer is
    // positional.
    let nodes: Vec<NodeRef> = held.items.iter().map(|i| i.node.clone()).collect();

    // ── before: addressable, and not readable ────────────────────────
    let before = resolve(&alice, &nodes).await;
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].node, mine);
    assert_eq!(before[0].reach, Reach::Local);
    assert_eq!(
        before[1].reach,
        Reach::NotPermitted,
        "writing the guest's domain into a setlist granted the studio a read"
    );
    assert_eq!(
        before[1].rel_path, "",
        "the refusal carried the path of a chart the reader may not have"
    );

    // ── the subscription, taken on the way a person takes one ────────
    alice
        .wiki_subscriptions()
        .await
        .subscribe(Subscriber::Vault, source_of(GUEST_DOMAIN))
        .await
        .expect("ACME subscribes to the guest's charts");

    let during = resolve(&alice, &nodes).await;
    assert_eq!(during[0].reach, Reach::Local, "the local half did not move");
    assert_eq!(
        during[1].reach,
        Reach::Reachable,
        "the subscription is what admits the reader, and it did not"
    );
    assert_eq!(during[1].org, GUEST_ORG, "the answer names the publisher");
    assert_eq!(
        during[1].rel_path, "resources/charts/hosanna.kf",
        "a reachable reference says where the content sits in its own org"
    );

    // ── after: the same reference, refused again ─────────────────────
    //
    // `force`, because dropping a subscription without it first
    // reconciles the local copy — a different question, asked in the
    // wiki chapters. What is under test here is that resolution
    // follows the subscription and nothing else.
    alice
        .wiki_subscriptions()
        .await
        .unsubscribe(
            Subscriber::Vault,
            format!("{GUEST_DOMAIN}/{CHART_LIBRARY}"),
            true,
        )
        .await
        .expect("ACME drops the subscription");

    let after = resolve(&alice, &nodes).await;
    assert_eq!(after[0].reach, Reach::Local);
    assert_eq!(
        after[1].reach,
        Reach::NotPermitted,
        "the chart stayed readable after the subscription that admitted it was dropped"
    );
    assert_eq!(after[1].rel_path, "");

    // The setlist itself is untouched by any of this: an unresolvable
    // item is a state a reader is told about, never a broken list.
    let still = alice
        .collections()
        .await
        .get(setlist.id)
        .await
        .expect("read the setlist")
        .expect("it is there")
        .items;
    assert_eq!(still.len(), 2);
    assert_eq!(still[1].node, theirs);
}

/// The boundary the ADR records, asserted rather than described: an org
/// on **another server** is addressable and not reachable, and a
/// subscription to it is refused at the moment it is asked for.
#[tokio::test]
async fn an_org_on_another_server_parses_and_does_not_resolve() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let victor = s.as_victor().await;

    // VNT really does hold the chart — so "not reachable" cannot be
    // confused with "not there".
    let theirs = victor
        .resources()
        .await
        .upsert_chart(chart("Reel Theme", "[Verse]\n| Am | F | C | G |\n"))
        .await
        .expect("VNT saves a chart on its own server");
    assert_eq!(theirs.slug, "reel-theme");

    let node = NodeRef::new(NodeKind::Chart, "reel-theme").in_domain("vnt.test");
    let answers = alice
        .links()
        .await
        .resolve_nodes(vec![node])
        .await
        .expect("resolve");
    assert_eq!(
        answers[0].reach,
        Reach::NotPermitted,
        "a reference to another server's org resolved — `LocalHomes` only \
         answers for orgs on the reader's own data root, and the remote \
         upstream is the next piece of work (ADR 0003)"
    );

    // And the subscription that would admit it is refused where it is
    // asked for, rather than accepted and left to fail on every read.
    let refused = alice
        .wiki_subscriptions()
        .await
        .subscribe(Subscriber::Vault, source_of("vnt.test"))
        .await;
    assert!(
        refused.is_err(),
        "subscribing to a source this server cannot serve was accepted: {refused:?}"
    );
}
