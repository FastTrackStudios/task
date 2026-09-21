//! Chapter — a session's audio, read from the server that holds it.
//!
//! This is the shape the sibling apps need and the one `VISION.md`
//! § "The rig loads from the cloud" commits to: Session is handed a
//! reference to a track, and the audio opens, on a machine that has
//! never held it.
//!
//! # The two halves, and which one was missing
//!
//! ADR 0003 splits an asset: the manifest says what it *is* and lives
//! under `resources/`, small enough to cross an org boundary; a
//! `ContentRef { root_id, path }` says where the bytes are, in the Files
//! layer that owns versioning, selective sync, renditions and chunked
//! streaming. That split is what makes a shared library usable —
//! subscribing moves names, not gigabytes.
//!
//! Both halves already worked. Bytes cross a server boundary through
//! `offer` / `accept`, and an accepted offer is *an ordinary root*
//! (`files.topology.federation`), so every lane after that point is
//! addressing something local. `collaboration.rs` proves the browse and
//! `device.rs` proves selective sync over it.
//!
//! What was missing was the join. A `ContentRef` written by ACME names
//! **ACME's** root id. Carried to VNT — the whole point of a manifest
//! being small — it arrives naming a root VNT does not have, and used as
//! a local id it resolves to nothing. Which reads as "the audio is
//! missing", when the truth is "that id belongs to another server".
//!
//! `Remote::origin_root` was always the fact that fixes it. Nothing
//! looked a root up *by* it. `federation::adopted_from` does, and this
//! chapter is the claim that the resulting round trip reaches real bytes.

use files::path::RootPath;
use files::service::access::Capability;
use files::service::federation::{EndpointId, adopted_from};
use integration::scenario::Scenario;
use resources_proto::{ContentRef, SampleDoc};

/// A take, standing in for a session's audio. Small deliberately — this
/// chapter is about reaching the bytes across a boundary, and `scale.rs`
/// is where size is the subject.
const TAKE: &[u8] = b"RIFF....WAVEfmt lead vocal, comp 3, bars 17-32";

fn sample(title: &str, content: ContentRef) -> SampleDoc {
    SampleDoc {
        slug: String::new(),
        title: title.into(),
        tags: vec!["vocal".into(), "comp".into()],
        duration_secs: 16,
        sample_rate: 48_000,
        body: "Comped from takes 2 and 5.".into(),
        content,
        updated_at: "2026-09-19T10:00:00Z".into(),
    }
}

/// ACME's session root, with the audio in it, readable by Alice.
///
/// Arranged through the backend like every other setup here; everything
/// asserted below goes over the wire.
async fn session_with_audio(s: &Scenario) -> (files::RootId, RootPath) {
    let tree = s.orgs.acme.tree().join("Session");
    std::fs::create_dir_all(tree.join("Audio Files")).expect("the session folder");
    std::fs::write(tree.join("Audio Files").join("lead-vocal.wav"), TAKE).expect("the take");
    let root = integration::orgs::adopt(&s.orgs.acme, "Session").await;
    // A freshly adopted root is nobody's: access is on the content
    // (`files.access.granularity`), so the person about to read it has
    // to be given it.
    files::service::access::AccessService::grant(
        &s.orgs.acme.backend,
        s.people.alice.subject.clone(),
        root,
        RootPath::root(),
        task_server::example_org::Holds::Owner.capabilities(),
    )
    .await
    .expect("ACME grants Alice the session");
    (
        root,
        RootPath::parse("Audio Files/lead-vocal.wav").expect("a root-relative path"),
    )
}

/// t[verify files.topology.federation]
///
/// The whole journey: ACME binds a take, offers the session to VNT, and
/// VNT — holding only the reference — reads the bytes.
#[tokio::test]
async fn a_foreign_content_ref_reaches_its_bytes_through_the_accepted_root() {
    let s = Scenario::open().await;
    let (acme_session, take_path) = session_with_audio(&s).await;
    let alice = s.as_alice().await;

    // ── ACME writes the binding, the way Signal or Session would ─────
    let bound = ContentRef {
        root_id: acme_session.to_string(),
        path: take_path.to_string(),
    };
    alice
        .resources()
        .await
        .upsert_sample(sample("Lead Vocal Comp", bound.clone()))
        .await
        .expect("declare the take");

    // ── the offer, carried as a message ──────────────────────────────
    //
    // "An offer is inert until accepted, and carrying it is a message,
    // not a protocol" — so this hop stands for however the two orgs
    // actually pass it, and the chapter asserts what happens after.
    let offer = alice
        .federation()
        .await
        .offer(
            acme_session,
            RootPath::parse("Audio Files").expect("the offered subtree"),
            EndpointId(s.orgs.vnt.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("ACME offers the session audio to VNT");

    let victor = s.as_victor().await;
    let accepted = victor
        .federation()
        .await
        .accept(offer)
        .await
        .expect("VNT accepts");

    // ── the join: a foreign id, resolved to the local root ───────────
    //
    // This is the step that did not exist. VNT holds `bound`, whose
    // `root_id` is ACME's, and has to discover that it already has an
    // accepted root standing for it.
    let remotes = victor
        .federation()
        .await
        .remotes()
        .await
        .expect("what VNT has accepted");
    let foreign: files::RootId = bound
        .root_id
        .parse::<uuid::Uuid>()
        .expect("a ContentRef carries a uuid")
        .into();
    let local = adopted_from(&remotes, foreign).expect(
        "VNT accepted an offer of exactly this root and could not find it \
         by the id the manifest names",
    );
    assert_eq!(
        local, accepted.root_id,
        "resolved to a different root than the one just accepted"
    );

    // ── and the path, rebased by the server that knows the subtree ───
    //
    // The offer was of `Audio Files`, so the accepted root begins there
    // and the manifest's `Audio Files/lead-vocal.wav` is `lead-vocal.wav`
    // inside it. This chapter used to strip that prefix by hand — which
    // only worked because its author knew what had been offered. An app
    // does not: `Remote` does not carry the offered path. So the server,
    // which kept it, does the rebase, and the app asks once.
    let resolved = victor
        .federation()
        .await
        .resolve_content(
            foreign,
            RootPath::parse(&bound.path).expect("the manifest's path"),
        )
        .await
        .expect("resolve")
        .expect("VNT accepted an offer containing this path");
    assert_eq!(resolved.root_id, local, "resolved into a different root");
    assert_eq!(resolved.path.as_str(), "lead-vocal.wav", "rebased wrongly");

    // ── and the bytes are really there ───────────────────────────────
    //
    // Through the ordinary media lane, against what is now an ordinary
    // root.
    let ticket = victor
        .media()
        .await
        .read(resolved.root_id, resolved.path)
        .await
        .expect("a ticket for the take on the accepted root");
    // `Some` is half the claim: the length is optional because a relayed
    // read need not know it, so this says the origin answered with the
    // size as well as the bytes — which is what lets a player show a
    // duration before the first chunk lands.
    assert_eq!(
        ticket.length,
        Some(TAKE.len() as u64),
        "the ticket describes something other than the take that was written"
    );
}

/// The negative half: without an accepted offer the reference is
/// unreachable, and says so as `None`.
///
/// Worth its own test because the failure it rules out is silent. If a
/// foreign `root_id` were ever treated as local, this case would return
/// a root that does not exist and the caller would report a missing
/// file — sending somebody to look for audio that is exactly where it
/// should be, on a server they never asked for access to.
#[tokio::test]
async fn an_unoffered_root_is_unreachable_rather_than_missing() {
    let s = Scenario::open().await;
    let (acme_session, _) = session_with_audio(&s).await;

    let victor = s.as_victor().await;
    let remotes = victor
        .federation()
        .await
        .remotes()
        .await
        .expect("VNT's remotes");

    assert_eq!(
        adopted_from(&remotes, acme_session),
        None,
        "VNT resolved a root nobody offered it"
    );
    // And the server-side resolver says the same, over the wire: `None`,
    // not a fault and not a path to nothing.
    assert_eq!(
        victor
            .federation()
            .await
            .resolve_content(
                acme_session,
                RootPath::parse("Audio Files/lead-vocal.wav").expect("path"),
            )
            .await
            .expect("resolving is not an error"),
        None,
        "an unoffered root resolved"
    );
}

/// **Signal's journey, whole**: a sample library taken by subscription
/// from another server, and the audio its manifest names reached by
/// offer — the two halves ADR 0003 split an asset into, rejoined on the
/// far side in one call.
///
/// `sibling_apps.rs` proves the manifest half crosses (the patch library
/// lands and resolves). This is what an app does next with a *sample*:
/// read the `ContentRef` out of the copy, and play what it points at.
///
/// # The subtree is the point
///
/// ACME offers `Samples/Kicks`, not the whole library root. So a kick
/// resolves — rebased onto the accepted root — and a snare in the folder
/// beside it does **not**, even though VNT holds both manifests. Holding
/// a manifest is holding a name; the offer is what grants the bytes, and
/// only the bytes it covers.
///
/// t[verify files.topology.federation] — the receiving side reaches
/// exactly the offered subtree through an ordinary root.
#[tokio::test(flavor = "multi_thread")]
async fn a_subscribed_sample_plays_from_the_offered_subtree_and_nothing_beside_it() {
    use wiki_proto::service::subscriptions::SourceGrant;
    use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

    const KICK: &[u8] = b"RIFF....WAVEfmt room kick, take two";
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let victor = s.as_victor().await;

    // ── ACME's sample folder: kicks and snares, adopted as one root ──
    let tree = s.orgs.acme.tree().join("Samples");
    std::fs::create_dir_all(tree.join("Kicks")).expect("kicks");
    std::fs::create_dir_all(tree.join("Snares")).expect("snares");
    std::fs::write(tree.join("Kicks/room-kick.wav"), KICK).expect("the kick");
    std::fs::write(tree.join("Snares/crack.wav"), b"RIFF....snare").expect("the snare");
    let samples_root = integration::orgs::adopt(&s.orgs.acme, "Samples").await;
    files::service::access::AccessService::grant(
        &s.orgs.acme.backend,
        s.people.alice.subject.clone(),
        samples_root,
        RootPath::root(),
        task_server::example_org::Holds::Owner.capabilities(),
    )
    .await
    .expect("ACME grants Alice the samples");

    // ── Signal declares both, each bound to its audio ────────────────
    let resources = alice.resources().await;
    for (title, path) in [
        ("Room Kick", "Kicks/room-kick.wav"),
        ("Crack Snare", "Snares/crack.wav"),
    ] {
        resources
            .upsert_sample(sample(
                title,
                ContentRef {
                    root_id: samples_root.to_string(),
                    path: path.into(),
                },
            ))
            .await
            .unwrap_or_else(|e| panic!("declare {title}: {e:?}"));
    }

    // ── VNT takes the sample library across the boundary ─────────────
    let secret = alice
        .wiki_subscriptions()
        .await
        .grant_source_read(SourceKind::Resource, "samples".to_owned())
        .await
        .expect("ACME grants read on its sample library");
    let subs = victor.wiki_subscriptions().await;
    subs.trust_source(SourceGrant {
        domain: "acme.test".to_owned(),
        endpoint: s.orgs.acme.endpoint.id().to_string(),
        kind: SourceKind::Resource,
        slug: "samples".to_owned(),
        secret,
    })
    .await
    .expect("VNT records the grant");
    subs.subscribe(
        Subscriber::Vault,
        Subscription {
            domain: "acme.test".into(),
            slug: "samples".into(),
            kind: SourceKind::Resource,
            title: "samples".into(),
            core: false,
            declined: false,
            selection: Default::default(),
        },
    )
    .await
    .expect("subscribe");
    subs.refresh_subscription(Subscriber::Vault, "acme.test/samples".to_owned())
        .await
        .expect("the manifests cross");

    // The copy carries ACME's root id, exactly as written — which is the
    // whole reason a resolver is needed: here, that id names nothing.
    let copy = s
        .orgs
        .vnt
        .org_root()
        .join("subscribed/acme.test/samples/room-kick/sample.md");
    let manifest = std::fs::read_to_string(&copy)
        .unwrap_or_else(|e| panic!("{} should be on VNT's disk: {e}", copy.display()));
    assert!(
        manifest.contains(&samples_root.to_string()),
        "the manifest arrived without the ContentRef that names its audio: {manifest}"
    );

    // ── ACME offers the kicks, and only the kicks ────────────────────
    let offer = alice
        .federation()
        .await
        .offer(
            samples_root,
            RootPath::parse("Kicks").expect("the offered subtree"),
            EndpointId(s.orgs.vnt.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("ACME offers its kick folder");
    let accepted = victor
        .federation()
        .await
        .accept(offer)
        .await
        .expect("VNT accepts");

    // ── the kick: one call from manifest to playable bytes ───────────
    let federation = victor.federation().await;
    let kick = federation
        .resolve_content(
            samples_root,
            RootPath::parse("Kicks/room-kick.wav").expect("path"),
        )
        .await
        .expect("resolve")
        .expect("the kick is inside what was offered");
    assert_eq!(kick.root_id, accepted.root_id);
    assert_eq!(kick.path.as_str(), "room-kick.wav");
    let ticket = victor
        .media()
        .await
        .read(kick.root_id, kick.path)
        .await
        .expect("a ticket for the kick");
    assert_eq!(
        ticket.length,
        Some(KICK.len() as u64),
        "not the kick's bytes"
    );

    // ── the snare: a manifest VNT holds, bytes it was never offered ──
    assert_eq!(
        federation
            .resolve_content(
                samples_root,
                RootPath::parse("Snares/crack.wav").expect("path"),
            )
            .await
            .expect("resolving is not an error"),
        None,
        "a file outside the offered subtree resolved — holding a manifest \
         is holding a name, and the offer is what grants the bytes"
    );
}
