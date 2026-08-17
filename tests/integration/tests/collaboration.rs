//! Chapter two — the two companies work on one project.
//!
//! ACME offers its session subtree to VNT over iroh; VNT accepts it and
//! browses it as an ordinary root. Then the grant is withdrawn from the
//! originating side, the project is composed from both halves as one
//! tree, and finally VNT *streams* a file it does not hold — the half of
//! `files.peering.serving` that says a host without the content fetches
//! it from a host that has it.
//!
//! Nothing here is one org talking to itself. Every call crosses a
//! company boundary, which is the only reason the chapter is worth
//! having: the failure these tests exist to catch is a lane that works
//! because both sides happen to share a process.

use files::RootId;
use files::path::RootPath;
use files::service::access::Capability;
use files::service::federation::{EndpointId, FederationService};
use files::service::media::MediaService;
use files::service::tree::TreeService;

use integration::scenario::{Scenario, pin};

/// ACME offers its takes to VNT, and VNT accepts.
///
/// The secret in the offer stands for the grant: a receiver holds a
/// string, not a key to ACME's disk, and every call it makes is checked
/// against the grant at the origin rather than against anything it
/// keeps locally.
async fn share_takes(s: &Scenario) -> (files::service::federation::Offer, RootId) {
    let offer = s
        .orgs
        .acme
        .backend
        .offer(
            s.acme_root,
            RootPath::parse("Audio Files").unwrap(),
            EndpointId(s.orgs.vnt.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("offer the takes to VNT");
    let accepted = s
        .orgs
        .vnt
        .backend
        .accept(offer.clone())
        .await
        .expect("VNT accepts");
    (offer, accepted.root_id)
}

// t[verify files.topology.federation]
#[tokio::test]
async fn an_accepted_offer_browses_like_any_other_root() {
    let s = Scenario::open().await;
    let (_offer, sessions) = share_takes(&s).await;

    // The same call VNT makes against its own content. That it does not
    // know this root is not local is the entire claim.
    let entries = s
        .orgs
        .vnt
        .backend
        .browse(sessions, RootPath::root())
        .await
        .expect("browse ACME's subtree from VNT");
    let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert!(
        names.contains(&"vox.wav") && names.contains(&"kick.wav"),
        "{names:?}"
    );
    // Offered subtree, not offered root: the session internals above
    // `Audio Files` are not VNT's business and must not appear.
    assert!(!names.contains(&"Song.rpp"), "{names:?}");
}

// t[verify files.topology.federation]
#[tokio::test]
async fn a_grant_is_withdrawn_at_the_origin_and_binds_on_the_next_call() {
    let s = Scenario::open().await;
    let (offer, sessions) = share_takes(&s).await;
    assert!(
        s.orgs
            .vnt
            .backend
            .browse(sessions, RootPath::root())
            .await
            .is_ok(),
        "fixture: it works before the withdrawal"
    );

    s.orgs
        .acme
        .backend
        .withdraw(offer.grant)
        .await
        .expect("withdraw");

    // Not "VNT deletes its copy of the grant" — revocation that depends
    // on the receiver cooperating is not revocation.
    assert!(
        s.orgs
            .vnt
            .backend
            .browse(sessions, RootPath::root())
            .await
            .is_err(),
        "a withdrawn offer still served content"
    );
}

// t[verify project.location.degraded]
#[tokio::test]
async fn an_unreachable_origin_costs_its_own_content_and_nothing_else() {
    let s = Scenario::open().await;
    let (offer, sessions) = share_takes(&s).await;
    s.orgs.acme.backend.withdraw(offer.grant).await.unwrap();
    assert!(
        s.orgs
            .vnt
            .backend
            .browse(sessions, RootPath::root())
            .await
            .is_err()
    );

    // VNT's own footage is untouched by ACME's state, and ACME's own
    // sessions are untouched by having stopped sharing them.
    s.orgs
        .vnt
        .backend
        .browse(s.vnt_root, RootPath::root())
        .await
        .expect("VNT's own root");
    s.orgs
        .acme
        .backend
        .browse(s.acme_root, RootPath::root())
        .await
        .expect("ACME's own root");
}

/// One project, both companies, one tree.
///
/// Neither half is the project's "real" home. That is the clause
/// [`files_domain::Composition`] has no field for, and the reason a
/// composition is a list of members rather than a root with attachments.
// t[verify project.location.composed]
#[tokio::test]
async fn a_project_composes_from_two_servers() {
    let s = Scenario::open().await;
    let (_offer, sessions) = share_takes(&s).await;

    let mut project = files_domain::Composition::new();
    project
        .with(files_domain::Member {
            name: "Sessions".into(),
            root: sessions,
            path: RootPath::root(),
        })
        .expect("ACME's half");
    project
        .with(files_domain::Member {
            name: "Footage".into(),
            root: s.vnt_root,
            path: RootPath::parse("Proxies").unwrap(),
        })
        .expect("VNT's half");
    assert_eq!(project.locations(), 2);

    // Each part resolves to whichever root answers for it — one of them
    // on another company's server — and the caller makes the same call
    // either way.
    let mut composed = Vec::new();
    for member in project.members() {
        let at = RootPath::parse(&member.name).unwrap();
        let located = project.locate(&at).expect("locate");
        let entries = s
            .orgs
            .vnt
            .backend
            .browse(located.member.root, located.within.clone())
            .await
            .expect("browse a member");
        for entry in entries {
            composed.push(format!("{}/{}", member.name, entry.name));
        }
    }
    composed.sort();

    assert!(
        composed.contains(&"Sessions/vox.wav".to_string()),
        "{composed:?}"
    );
    assert!(
        composed.contains(&"Footage/reel.mov".to_string()),
        "{composed:?}"
    );
}

/// VNT plays a file it holds none of.
///
/// `files.peering.serving`: a host without the content answers anyway,
/// fetching from the host that has it. The caller makes the ordinary
/// `read` call and gets an ordinary local ticket — the relay is not part
/// of its vocabulary.
// t[verify files.peering.serving]
#[tokio::test]
async fn a_deliverable_streams_across_the_company_boundary() {
    let s = Scenario::open().await;
    // The byte lane reads the pinned head, so ACME pins before sharing.
    pin(&s.orgs.acme, s.acme_root, "the takes as delivered").await;
    let (_offer, sessions) = share_takes(&s).await;

    let ticket = s
        .orgs
        .vnt
        .backend
        .read(sessions, RootPath::parse("vox.wav").unwrap())
        .await
        .expect("a ticket for content this server does not hold");

    let mut played = Vec::new();
    s.orgs
        .vnt
        .backend
        .redeem_bytes(&ticket.token, None, &mut played)
        .await
        .expect("redeem");
    assert_eq!(played, b"vox take one");
}

/// A preview seeks.
///
/// The whole point of a relayed ticket being seekable is that scrubbing
/// a 4 GB reel transfers the part you scrubbed to, not the part before
/// it. A relay that could only stream from zero would make the feature
/// technically present and useless.
// t[verify files.peering.serving]
#[tokio::test]
async fn a_relayed_ticket_serves_a_range_rather_than_the_whole_file() {
    let s = Scenario::open().await;
    pin(&s.orgs.acme, s.acme_root, "the takes as delivered").await;
    let (_offer, sessions) = share_takes(&s).await;

    let ticket = s
        .orgs
        .vnt
        .backend
        .read(sessions, RootPath::parse("vox.wav").unwrap())
        .await
        .expect("ticket");

    let mut scrubbed = Vec::new();
    s.orgs
        .vnt
        .backend
        .redeem_bytes(&ticket.token, Some((4, 7)), &mut scrubbed)
        .await
        .expect("redeem a range");
    assert_eq!(
        scrubbed,
        b"take",
        "the range landed at the wrong offset: {:?}",
        String::from_utf8_lossy(&scrubbed)
    );
}
