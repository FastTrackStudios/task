#![allow(clippy::large_futures)]
//! Chapter — **two companies on two servers subscribe to each other's
//! sources**, and each copy goes on reading when the grant is withdrawn.
//!
//! This is the half of ADR 0003 that `setlist.rs` recorded as the
//! boundary: a source on another server was unreachable because
//! `LocalOrgs` answers only for orgs on the reader's own data root.
//! `song_library.rs` is the sibling chapter on one disk — `start_beside`
//! puts both orgs under one root, which is the arrangement `admin seed`
//! produces and the one every federated chapter before this used.
//!
//! The chapter runs both directions on purpose: ACME takes VNT's wiki,
//! VNT takes ACME's song library, and a reference into that library
//! resolves from VNT's own copy. One direction would leave "it works when
//! the publisher is the org that happens to hold the seed's wikis" as an
//! untested assumption.
//!
//! [`integration::orgs::Orgs`] is two servers on two disks, so it is the
//! only harness in which "another server" means anything, and this is the
//! chapter that needs it.
//!
//! # The three facts that make it work, and where each one lives
//!
//! **The publisher answers.** `Subscriptions::source_manifest` /
//! `source_file` are on the anonymous surface, because the caller is a
//! server rather than a person and holds no session here. A secret the
//! publisher minted authenticates it; the source's own visibility
//! authorises the read, checked by the same `admits` a subscriber on the
//! publisher's disk goes through (`wiki.access.visibility`).
//!
//! **The subscriber writes the grant down.** `trust_source` — two facts,
//! where the domain is and what reads it. Until then the domain names
//! nothing this server can reach and the subscription is an orphan, which
//! is exactly what it was before any of this existed.
//!
//! **The refresh is unchanged.** `materialize::refresh` has always been
//! generic over its source, so the same call that copies a sibling org's
//! wiki off disk copies this one off the wire.
//!
//! # A shelf crosses as well, and the line is size rather than kind
//!
//! A vault manifest is content-agnostic — the walk hashes every file it
//! meets, markdown or not — so an asset shelf's **documents** ride the
//! same engine. What a subscription cannot carry is a take: every fetch
//! is one whole file in one message, and ADR 0003's rule is that
//! subscribing moves names and not gigabytes. So a remote shelf refresh
//! is bounded (`materialize::REMOTE_FILE_LIMIT`) and reports what it left
//! behind.
//!
//! A **project** is refused outright, and that is the same rule reaching
//! its conclusion rather than a missing walker: a project *is* its media.
//! Those bytes cross as a File Root — offered, accepted, pulled in chunks
//! by the lane that owns resumption and renditions — which is what
//! `remote_assets.rs` proves. A **Resource** is refused too: an edition is
//! installed into a corpus library, not pulled file by file.
//!
//! The last two tests pin both, so each stays a stated decision rather
//! than a surprise.

use integration::scenario::Scenario;
use links_proto::{NodeKind, NodeRef, Reach};
use wiki_proto::service::subscriptions::SourceGrant;
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

/// VNT's wiki, the one declared source in the seed that is owned by the
/// *other* company — `example_org::DECLARED_WIKIS`, unlisted, so it is
/// in no directory and subscribable by anyone holding the reference.
const POST_PRODUCTION: &str = "post-production";
const VNT_DOMAIN: &str = "vnt.test";

/// ACME's song shelf, and ACME's domain — the other direction, which is
/// the one the sibling apps need: a shared library of songs and charts,
/// read by whoever is working on the job.
const SONGS: &str = "songs";
const ACME_DOMAIN: &str = "acme.test";

/// ACME's private wiki. Private is a refusal for outsiders, and the
/// refusal is the interesting half of `wiki.access.visibility`.
const PRIVATE_WIKI: &str = "studio-research";

/// One page of the source, chosen because its content is a fact ACME
/// actually works from — the number a mix is measured against.
const SPEC_PAGE: &str = "Specs/Loudness Delivery.md";

fn source(slug: &str, kind: SourceKind) -> Subscription {
    Subscription {
        domain: VNT_DOMAIN.into(),
        slug: slug.into(),
        kind,
        title: slug.into(),
        core: false,
        declined: false,
        selection: Default::default(),
    }
}

/// Where the subscriber keeps what it took: `subscribed/<domain>/<slug>/`
/// — the same address a reference uses, whether the source was on this
/// disk or on another server. Nothing downstream of the refresh knows
/// which.
fn held_copy(org_root: &std::path::Path, slug: &str) -> std::path::PathBuf {
    org_root.join("subscribed").join(VNT_DOMAIN).join(slug)
}

/// The whole loop, in the order a person does it.
///
/// t[verify wiki.subscribe.federated] — the whole claim: same surface,
/// same ids, same reports, with only the latency different.
/// t[verify wiki.subscribe.local-copy] — across a server boundary: after
/// the refresh the pages are on the subscriber's own disk, so they read
/// with the publisher unreachable.
/// t[verify wiki.access.visibility] — a grant does not override
/// visibility, and withdrawing one stops the refreshing without touching
/// what is already held.
/// t[verify wiki.life.orphan] — a source that stops answering leaves a
/// copy that still resolves.
///
/// # Why this one asks for a multi-threaded runtime
///
/// Every other chapter here runs on the `#[tokio::test]` default, which
/// is a single-threaded runtime, and that is fine because nothing they do
/// blocks on the wire from inside the server. This does: a remote wiki
/// source is a sync `SourceVault` bridging onto an async client, which is
/// safe on the `spawn_blocking` thread a dispatched backend method runs
/// on and is refused outright when the runtime has only one thread to
/// block — see `task_server::federated_orgs`. A deployment is
/// multi-threaded (`#[tokio::main]`), so this asks for the runtime the
/// product has rather than asserting the guard.
#[tokio::test(flavor = "multi_thread")]
async fn a_wiki_on_another_server_is_subscribed_refreshed_and_then_orphaned() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let victor = s.as_victor().await;
    let acme_root = s.orgs.acme.org_root();

    // The source really is on the other server's disk, so "not
    // reachable" below cannot be confused with "not there".
    assert!(
        s.orgs
            .vnt
            .org_root()
            .join("wikis")
            .join(POST_PRODUCTION)
            .join("purpose.md")
            .is_file(),
        "the seed plants VNT's Post Production wiki on VNT's own root"
    );
    assert!(
        !s.orgs
            .acme
            .org_root()
            .join("wikis")
            .join(POST_PRODUCTION)
            .exists(),
        "and not on ACME's — this chapter would prove nothing on one disk"
    );

    let subs = alice.wiki_subscriptions().await;

    // ── with no grant, this server answers for its own disk ──────────
    //
    // And its own disk is all it can answer for. `wiki_domains` gives
    // every example org an `<name>.test` name whether or not that org is
    // here, so ACME recognises `vnt.test`, looks beside its own orgs, and
    // says there is no such wiki. Which is the answer `setlist.rs` pins
    // as the boundary: correct while this server is the only place a
    // source could be.
    let refused = subs
        .subscribe(Subscriber::Vault, source(POST_PRODUCTION, SourceKind::Wiki))
        .await
        .expect_err("with nothing recorded, ACME can only answer for ACME");
    assert!(
        refused.to_string().contains("no wiki"),
        "unexpected refusal: {refused}"
    );

    // ── VNT mints a grant ────────────────────────────────────────────
    //
    // A member's call on the publisher's side. What comes back is inert
    // — a string — and carrying it to ACME is a message, not a protocol.
    let secret = victor
        .wiki_subscriptions()
        .await
        .grant_source_read(SourceKind::Wiki, POST_PRODUCTION.to_owned())
        .await
        .expect("VNT grants read on its own wiki");
    assert!(!secret.is_empty(), "a grant with no secret grants nothing");

    // ── ACME writes it down ──────────────────────────────────────────
    //
    // Two facts: where `vnt.test` is, and what reads this source there.
    // The endpoint id is what a person would have been handed — no host,
    // no port, no certificate.
    subs.trust_source(SourceGrant {
        domain: VNT_DOMAIN.to_owned(),
        endpoint: s.orgs.vnt.endpoint.id().to_string(),
        kind: SourceKind::Wiki,
        slug: POST_PRODUCTION.to_owned(),
        secret: secret.clone(),
    })
    .await
    .expect("ACME records the grant it was given");
    let trusted = subs.trusted_sources().await.expect("what ACME can reach");
    assert_eq!(trusted.len(), 1);
    assert_eq!(trusted[0].domain, VNT_DOMAIN);
    assert_eq!(trusted[0].slug, POST_PRODUCTION);

    // ── and now the source is somewhere, so it can be taken on ───────
    //
    // The grant is what tells "there is no such wiki here" apart from
    // "that wiki is somewhere else": with one recorded, this server stops
    // answering for the source and says `Unknown` — orphan — because only
    // VNT can say whether ACME is admitted, and VNT says so on every
    // call.
    subs.subscribe(Subscriber::Vault, source(POST_PRODUCTION, SourceKind::Wiki))
        .await
        .expect("a source on another server may be taken on once it has a place");

    // ── and the refresh crosses the boundary ─────────────────────────
    let report = subs
        .refresh_subscription(Subscriber::Vault, format!("{VNT_DOMAIN}/{POST_PRODUCTION}"))
        .await
        .expect("the grant is recorded, so the source answers");
    assert!(
        report.pulled >= 2,
        "the wiki's pages should have come down: {report:?}"
    );

    let page = held_copy(&acme_root, POST_PRODUCTION).join(SPEC_PAGE);
    let text = std::fs::read_to_string(&page)
        .unwrap_or_else(|e| panic!("{} should be on ACME's disk now: {e}", page.display()));
    assert!(
        text.contains("LUFS"),
        "and it is VNT's page rather than a stub: {text}"
    );

    // ── VNT withdraws it ─────────────────────────────────────────────
    //
    // Binds on the next call, because the secret is checked on every one
    // — the only ordering a revocation across a boundary can honestly
    // promise.
    victor
        .wiki_subscriptions()
        .await
        .revoke_source_read(SourceKind::Wiki, POST_PRODUCTION.to_owned())
        .await
        .expect("VNT revokes");
    let refused = subs
        .refresh_subscription(Subscriber::Vault, format!("{VNT_DOMAIN}/{POST_PRODUCTION}"))
        .await
        .expect_err("a withdrawn grant stops the refreshing");
    assert!(
        refused.to_string().contains("no grant"),
        "a withdrawn grant should say so: {refused}"
    );

    // And the copy is untouched: what ended is the refreshing.
    assert!(
        std::fs::read_to_string(&page).is_ok_and(|t| t.contains("LUFS")),
        "revocation deleted the subscriber's copy — an orphan still reads \
         (`wiki.life.orphan`)"
    );
    let held = subs
        .list_subscriptions(Subscriber::Vault)
        .await
        .expect("list");
    assert!(
        held.iter()
            .any(|h| h.subscription.slug == POST_PRODUCTION && h.files > 0),
        "the subscription still names a copy with files in it: {held:?}"
    );
}

/// A secret does not widen what visibility allows — the two checks are
/// separate and both must hold.
///
/// t[verify wiki.access.visibility] — private is a refusal for anyone
/// outside the owning org, and it is refused at the point of *minting*,
/// so a publisher cannot hand out access that would never work.
#[tokio::test]
async fn a_private_wiki_cannot_be_granted_to_another_server_at_all() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    assert!(
        s.orgs
            .acme
            .org_root()
            .join("wikis")
            .join(PRIVATE_WIKI)
            .is_dir(),
        "the seed plants ACME's private wiki"
    );
    let refused = alice
        .wiki_subscriptions()
        .await
        .grant_source_read(SourceKind::Wiki, PRIVATE_WIKI.to_owned())
        .await
        .expect_err("a private wiki admits no outsider, so there is nothing to grant");
    let said = refused.to_string();
    assert!(
        said.contains("private") || said.contains("refus"),
        "the refusal should name the visibility, not a missing file: {said}"
    );
    // And nothing is left behind: a grant for something that cannot be
    // served would read as access somebody could use.
    let granted = alice
        .wiki_subscriptions()
        .await
        .grant_source_read(SourceKind::Wiki, PRIVATE_WIKI.to_owned())
        .await;
    assert!(granted.is_err(), "the second ask is refused the same way");
}

/// An asset shelf crosses too — its documents, not its stems.
///
/// The other direction from the wiki test above, and the direction the
/// sibling apps need: VNT takes ACME's song library, which is what a
/// shared library *is* for. `song_library.rs` is this chapter on one
/// disk; the only thing that differs here is where the two read calls
/// land.
///
/// t[verify wiki.subscribe.federated] — for an asset shelf.
#[tokio::test(flavor = "multi_thread")]
async fn an_asset_shelf_crosses_a_boundary_and_leaves_an_oversized_take_behind() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let victor = s.as_victor().await;

    // ACME's songs shelf is planted by the seed — two song documents,
    // `example_org::DECLARED_ASSETS`. A take is dropped on it here, far
    // over the wire bound, because "a shelf holds documents *and*
    // somebody's stems" is the ordinary case and the interesting one:
    // the documents must arrive anyway.
    let shelf = s.orgs.acme.org_root().join("assets").join(SONGS);
    assert!(
        shelf.join("track-one.md").is_file(),
        "the seed plants ACME's song shelf"
    );
    std::fs::create_dir_all(shelf.join("Stems")).expect("a stems folder");
    std::fs::write(shelf.join("Stems/lead vocal.wav"), vec![7u8; 9 << 20])
        .expect("a take bigger than a subscription carries");

    let secret = alice
        .wiki_subscriptions()
        .await
        .grant_source_read(SourceKind::Assets, SONGS.to_owned())
        .await
        .expect("a shelf is published by existing on the tier");

    let subs = victor.wiki_subscriptions().await;
    subs.trust_source(SourceGrant {
        domain: ACME_DOMAIN.to_owned(),
        endpoint: s.orgs.acme.endpoint.id().to_string(),
        kind: SourceKind::Assets,
        slug: SONGS.to_owned(),
        secret,
    })
    .await
    .expect("VNT records the grant");
    subs.subscribe(
        Subscriber::Vault,
        Subscription {
            domain: ACME_DOMAIN.into(),
            slug: SONGS.into(),
            kind: SourceKind::Assets,
            title: SONGS.into(),
            core: false,
            declined: false,
            selection: Default::default(),
        },
    )
    .await
    .expect("subscribe to the song library");

    let report = subs
        .refresh_subscription(Subscriber::Vault, format!("{ACME_DOMAIN}/{SONGS}"))
        .await
        .expect("the shelf's documents cross");
    assert!(
        report.pulled >= 2,
        "both song documents should have come down: {report:?}"
    );

    let copy = s
        .orgs
        .vnt
        .org_root()
        .join("subscribed")
        .join(ACME_DOMAIN)
        .join(SONGS);
    let song = std::fs::read_to_string(copy.join("track-one.md"))
        .expect("ACME's song document is on VNT's disk now");
    assert!(!song.trim().is_empty(), "the document arrived empty");

    // And the take did not come with it. Not an error and not a silent
    // omission: a subscription carries names, and the bytes of a take
    // cross as a File Root (`remote_assets.rs`).
    assert!(
        !copy.join("Stems/lead vocal.wav").exists(),
        "a 9 MiB take was pulled through a subscription — ADR 0003 says \
         names cross and gigabytes do not"
    );

    // ── and now the reference resolves ───────────────────────────────
    //
    // The boundary `setlist.rs` records, crossed: `acme.test/song:track-one`
    // is a reference into an org that is **not on VNT's disk**, and it
    // resolves — from VNT's own copy, which exists only because VNT
    // subscribed. The rule is untouched: a reference addresses, a
    // subscription authorises.
    let resolved = victor
        .links()
        .await
        .resolve_nodes(vec![
            NodeRef::new(NodeKind::Song, "track-one").in_domain(ACME_DOMAIN),
        ])
        .await
        .expect("resolve")
        .pop()
        .expect("one answer per node");
    assert_eq!(
        resolved.reach,
        Reach::Reachable,
        "a subscribed song on another server should resolve: {resolved:?}"
    );
    assert_eq!(
        resolved.rel_path, "subscribed/acme.test/songs/track-one.md",
        "the answer names the copy the refresh wrote"
    );
    assert!(
        s.orgs.vnt.org_root().join(&resolved.rel_path).is_file(),
        "and that path has to be a file that is actually there"
    );
}

/// The same reference, with the subscription dropped: back to a refusal.
///
/// The security claim of the qualified form, and it is only a claim until
/// something watches it *change*. `setlist.rs` does this for two orgs on
/// one disk; this is the same watch across a server boundary, where the
/// bytes are on the reader's own disk the whole time — so what is being
/// tested is genuinely the subscription and not the absence of a file.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_subscription_takes_the_reference_away_again() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let victor = s.as_victor().await;

    let secret = alice
        .wiki_subscriptions()
        .await
        .grant_source_read(SourceKind::Assets, SONGS.to_owned())
        .await
        .expect("ACME grants read on its song shelf");
    let subs = victor.wiki_subscriptions().await;
    subs.trust_source(SourceGrant {
        domain: ACME_DOMAIN.to_owned(),
        endpoint: s.orgs.acme.endpoint.id().to_string(),
        kind: SourceKind::Assets,
        slug: SONGS.to_owned(),
        secret,
    })
    .await
    .expect("recorded");
    let shelf = Subscription {
        domain: ACME_DOMAIN.into(),
        slug: SONGS.into(),
        kind: SourceKind::Assets,
        title: SONGS.into(),
        core: false,
        declined: false,
        selection: Default::default(),
    };
    subs.subscribe(Subscriber::Vault, shelf.clone())
        .await
        .expect("subscribe");
    subs.refresh_subscription(Subscriber::Vault, format!("{ACME_DOMAIN}/{SONGS}"))
        .await
        .expect("refresh");

    let song = NodeRef::new(NodeKind::Song, "track-one").in_domain(ACME_DOMAIN);
    let reach = |who: integration::client::Session, node: NodeRef| async move {
        who.links()
            .await
            .resolve_nodes(vec![node])
            .await
            .expect("resolve")
            .pop()
            .expect("one answer")
            .reach
    };
    assert_eq!(
        reach(s.as_victor().await, song.clone()).await,
        Reach::Reachable
    );

    // `force`, because the copy is upstream's content and VNT has not
    // pushed anything — the check exists for local work, and there is
    // none.
    subs.unsubscribe(Subscriber::Vault, format!("{ACME_DOMAIN}/{SONGS}"), true)
        .await
        .expect("drop the subscription");

    assert_eq!(
        reach(s.as_victor().await, song).await,
        Reach::NotPermitted,
        "the reference outlived the subscription that authorised it — the \
         copy is still on disk, which is exactly why this has to be \
         decided by the subscription and not by the filesystem"
    );
}

/// A project is still refused across a boundary, and says why.
///
/// Not an orphan, which would read as "the publisher is unreachable" and
/// send somebody to check a network that is fine. And not a missing
/// walker either: a project *is* its media, so the refusal names the
/// route those bytes take instead — a File Root, offered and accepted,
/// which is what `remote_assets.rs` proves.
#[tokio::test]
async fn a_project_on_another_server_names_the_gap_rather_than_reporting_an_orphan() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let victor = s.as_victor().await;
    let subs = alice.wiki_subscriptions().await;

    // VNT publishes its projects tier the way every org does — by having
    // one — so the grant is mintable and it is the *refresh* that draws
    // the line.
    // A directory on the tier is not a project: without a `project.md` it
    // is unclassified content and admits nobody
    // (`project.identity.declaration`), so this picks one that declares
    // itself.
    let projects = s.orgs.vnt.org_root().join("projects");
    let project = std::fs::read_dir(&projects)
        .expect("VNT has a Projects tier")
        .flatten()
        .filter(|e| e.path().join(org_proto::PROJECT_PAGE).is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .next()
        .expect("the seed declares at least one project on VNT");

    let secret = victor
        .wiki_subscriptions()
        .await
        .grant_source_read(SourceKind::Projects, project.clone())
        .await
        .expect("a project is published by existing on the tier");
    subs.trust_source(SourceGrant {
        domain: VNT_DOMAIN.to_owned(),
        endpoint: s.orgs.vnt.endpoint.id().to_string(),
        kind: SourceKind::Projects,
        slug: project.clone(),
        secret,
    })
    .await
    .expect("recorded");
    subs.subscribe(Subscriber::Vault, source(&project, SourceKind::Projects))
        .await
        .expect("subscribing to it is allowed");

    let refused = subs
        .refresh_subscription(Subscriber::Vault, format!("{VNT_DOMAIN}/{project}"))
        .await
        .expect_err("only a wiki crosses a server boundary today");
    assert!(
        refused.to_string().contains("File Root"),
        "the refusal should name the route a project's bytes take rather than \
         the network: {refused}"
    );
}
