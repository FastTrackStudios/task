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

    // ── and the bytes are really there ───────────────────────────────
    //
    // Through the ordinary media lane, against what is now an ordinary
    // root. The offer was of `Audio Files`, so the path inside the
    // accepted root is relative to what was offered — the receiver never
    // learns the subtree's parents.
    let inside = RootPath::parse("lead-vocal.wav").expect("path within the offered subtree");
    let ticket = victor
        .media()
        .await
        .read(local, inside)
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
}
