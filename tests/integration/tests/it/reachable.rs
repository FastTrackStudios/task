//! Chapter zero — can a client reach any of this at all.
//!
//! Every other chapter asks whether the answer is right. This one asks
//! the question underneath: whether a signed-in person, dialling a
//! server over the wire, gets an answer.
//!
//! It exists because both of the real failures in this feature were of
//! that shape and neither was visible to a test that called a backend
//! directly. One lane was implemented, unit-tested and never mounted on
//! the router. Two services shared a trait name, so mounting the second
//! silently unmounted the first. In both cases every existing test
//! passed.
//!
//! So: the real `org_layer_router`, `TASK_ENFORCE_PERMISSIONS=1`, and a
//! session token in per-call metadata. A lane missing from the router
//! or from the permit tables fails here.

use files::path::RootPath;
use files::service::roots::RootsServiceClient;
use files::service::tree::TreeServiceClient;

use integration::client::Session;
use integration::scenario::Scenario;

// t[verify files.topology.multi-server]
#[tokio::test]
async fn a_signed_in_owner_reaches_her_own_org_over_the_wire() {
    let s = Scenario::open().await;
    let alice = Session::open(&s.orgs.acme, s.people.alice.token.clone()).await;

    let roots: RootsServiceClient = alice.roots().await;
    let listed = roots
        .list()
        .await
        .expect("an owner's client could not list her own roots");
    assert!(
        listed.iter().any(|r| r.id == s.acme_root.get()),
        "the adopted root did not come back over the wire: {listed:?}"
    );
}

/// The tree lane, over the wire, as the person who owns it.
// t[verify files.topology.multi-server]
#[tokio::test]
async fn browsing_works_through_the_router_and_the_gate() {
    let s = Scenario::open().await;
    let alice = Session::open(&s.orgs.acme, s.people.alice.token.clone()).await;

    let tree: TreeServiceClient = alice.tree().await;
    let listing = tree
        .browse(s.acme_root, RootPath::root())
        .await
        .expect("browse over the wire");
    let names: Vec<&str> = listing.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Audio Files"), "{names:?}");
}

/// The negative half.
///
/// Without this the two tests above would pass identically on a server
/// that let everybody through, and the gate would be decorative. What
/// makes the assertion meaningful is that it is the *same call* the
/// owner just made successfully — the only difference is who asked.
// t[verify files.access.granularity]
#[tokio::test]
async fn an_anonymous_caller_is_refused() {
    let s = Scenario::open().await;
    let stranger = Session::anonymous(&s.orgs.acme).await;

    let roots: RootsServiceClient = stranger.roots().await;
    let refused = roots.list().await;
    assert!(
        refused.is_err(),
        "an anonymous caller listed an org's roots: {refused:?}"
    );
}
