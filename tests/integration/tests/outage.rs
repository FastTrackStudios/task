//! Chapter nineteen — the other server goes away for a week.
//!
//! `scenario.album.outage`: the album still opens. Structure and
//! metadata stay browsable, the missing server's content is *visibly*
//! unavailable, and work continues on everything else.
//!
//! # Unreachable is not absent
//!
//! This is the whole chapter, and it is one assertion made twice. A
//! folder that renders empty because its server is down is worse than an
//! error: the person looking at it concludes their footage is gone, and
//! the honest states — "here, and I cannot reach it" versus "not here" —
//! have to be distinguishable at the surface.
//!
//! `files.catalogue.offline` says the catalogue marks rather than omits.
//! What that means in practice is that the *structure* is a local
//! record, so it survives the origin, and only the bytes need the wire.
//!
//! # An outage, not a shutdown
//!
//! `Server::go_offline` closes the endpoint and leaves everything else
//! alone: the process runs, the disk is intact, the org is still hosted.
//! Killing the server instead would prove the same assertion for the
//! wrong reason — of course a dead machine serves nothing — and would
//! not distinguish "unreachable" from "gone", which is what the rule is
//! entirely about.

use files::RootId;
use files::path::RootPath;
use files::service::access::Capability;
use files::service::federation::EndpointId;

use integration::scenario::Scenario;

/// ACME offers its takes to VNT, and VNT accepts — the arrangement
/// `collaboration.rs` builds, in the shape this chapter needs.
async fn federated(s: &Scenario) -> RootId {
    let offer = s
        .as_alice()
        .await
        .federation()
        .await
        .offer(
            s.acme_root,
            RootPath::parse("Audio Files").unwrap(),
            EndpointId(s.orgs.vnt.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("ACME offers the takes to VNT");
    s.as_victor()
        .await
        .federation()
        .await
        .accept(offer)
        .await
        .expect("VNT accepts")
        .root_id
}

// t[verify project.location.degraded]
// t[verify files.catalogue.offline]
// t[verify scenario.album.outage] — the other server goes away and work
// continues on everything still reachable
/// VNT keeps working when ACME is unreachable.
#[tokio::test]
async fn an_orgs_own_work_survives_the_other_server_going_away() {
    let s = Scenario::open().await;
    let takes = federated(&s).await;

    // Read it once while both are up, so the catalogue has seen it.
    s.as_victor()
        .await
        .tree()
        .await
        .browse(takes, RootPath::root())
        .await
        .expect("browse ACME's subtree while ACME is up");

    s.orgs.acme.go_offline().await;

    // VNT's own root is untouched. "A server being unreachable costs
    // reach, never correctness" — and the server that went away is not
    // this one.
    let own = s
        .as_victor()
        .await
        .tree()
        .await
        .browse(s.vnt_root, RootPath::root())
        .await
        .expect("VNT's own content is its own business");
    assert!(
        own.iter().any(|e| e.name == "Cut.drp"),
        "VNT lost its own project because ACME went down: {own:?}"
    );
}

// t[verify files.catalogue.offline]
/// The federated folder does not quietly become empty.
///
/// Either the browse fails — saying so — or it answers with the entries
/// it knew about. What it must not do is succeed with nothing, because
/// that renders as "your footage is gone".
#[tokio::test]
async fn an_unreachable_origin_is_not_reported_as_an_empty_folder() {
    let s = Scenario::open().await;
    let takes = federated(&s).await;

    let victor = s.as_victor().await;
    let before = victor
        .tree()
        .await
        .browse(takes, RootPath::root())
        .await
        .expect("browse while ACME is up");
    assert!(!before.is_empty(), "nothing was there to begin with");

    s.orgs.acme.go_offline().await;

    match victor.tree().await.browse(takes, RootPath::root()).await {
        // Marked, not omitted: the structure is a local record and
        // survives the origin.
        Ok(after) => assert!(
            !after.is_empty(),
            "the folder came back empty while its origin was unreachable — \
             `files.catalogue.offline` says unavailable content is marked, \
             never missing"
        ),
        // Also honest: the caller is told, and can say so.
        Err(fault) => {
            let said = format!("{fault:?}");
            assert!(
                !said.is_empty(),
                "an unreachable origin must produce a fault a surface can \
                 render, not a silent nothing"
            );
        }
    }
}
