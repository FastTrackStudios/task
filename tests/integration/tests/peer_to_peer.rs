//! Chapter ten — two clients, no server in the middle.
//!
//! `files.topology.multi-server` says "where two peers can reach each
//! other, bytes move directly over iroh/QUIC". Every other chapter has
//! one of those peers being a server. This one has neither.
//!
//! Which is the case that actually matters in a studio: two laptops on
//! the same switch, one holding the session the other needs. Sending it
//! through a server means the bytes leave the building and come back, and
//! for a 244 GB project that is the difference between lunch and a day.
//!
//! # What a client has to be
//!
//! A peer, not a puller. Each laptop here binds an iroh endpoint, serves
//! the replica lane on it, and admits the endpoints it will answer — the
//! same three things a server does, minus the accounts, because a laptop
//! has nobody to authenticate. Its endpoint key doubles as its
//! `files.device.identity`.

use files_domain::Hosting;

use integration::device::Laptop;
use integration::scenario::Scenario;

/// One laptop pulls a project from another, with no server consulted.
// t[verify files.topology.multi-server]
#[tokio::test]
async fn a_laptop_pulls_a_project_from_another_laptop() {
    let holder = Laptop::open().await;
    let receiver = Laptop::empty_peer(holder.album).await;
    let tree = receiver.tree.clone();

    // The holder admits the receiver's endpoint. That is the whole
    // credential: no account, no token, no session — an endpoint the
    // transport will prove and a list saying it may ask.
    holder
        .backend
        .admit_host(receiver.host_id(), Hosting::working());

    let peer = receiver.dial_replica(&holder.endpoint).await;
    let report = files_sync::reconcile(&receiver.backend, &peer, holder.album.get())
        .await
        .expect("pull from the other laptop");

    assert!(report.chunks_fetched > 0, "the pull moved nothing");
    let landed = std::fs::read(tree.join("Sessions").join("Audio Files").join("vox.wav"))
        .expect("the take landed on the receiving laptop");
    assert_eq!(landed, b"vox take one");
}

/// And an unadmitted laptop cannot.
///
/// Identical to the test above but for the admitting line, which is the
/// only way to know admission is what authorised the pull rather than
/// the endpoint being able to ask at all.
// t[verify files.peering.scope]
#[tokio::test]
async fn an_unadmitted_laptop_is_refused() {
    let holder = Laptop::open().await;
    let receiver = Laptop::empty_peer(holder.album).await;

    let peer = receiver.dial_replica(&holder.endpoint).await;
    let refused = peer.heads(holder.album.get()).await;

    assert!(
        refused.is_err(),
        "an unadmitted laptop read another's heads: {refused:?}"
    );
}

/// A device serves the replica lane and nothing else.
///
/// The narrowness is what makes it safe for a laptop to answer at all. A
/// peer needs the commit graph and the chunks under it; it does not get to
/// answer for the org — browsing a tree, reading grants, accepting writes
/// are questions for the server that holds it, and a device answering them
/// would be a second authority nobody registered.
// t[verify files.peering.scope]
#[tokio::test]
async fn a_device_does_not_answer_for_the_org() {
    let holder = Laptop::open().await;
    let receiver = Laptop::empty_peer(holder.album).await;
    holder
        .backend
        .admit_host(receiver.host_id(), Hosting::working());

    // Admitted, and still not able to reach a lane the device does not
    // serve: the tree lane is not mounted on a device at all, so this
    // fails at dispatch rather than at the gate.
    let link = architect::iroh_link::connect(&receiver.endpoint, holder.endpoint.id())
        .await
        .expect("dial");
    let tree: files::TreeServiceClient = vox_core::initiator_on(link)
        .establish()
        .await
        .expect("establish");
    let refused = tree
        .browse(holder.album, files::path::RootPath::root())
        .await;
    assert!(
        refused.is_err(),
        "a device answered a tree browse: {refused:?}"
    );
}

/// The server holds the canonical copy, and neither laptop needs it
/// reachable to sync with the other.
///
/// This is the property the chapter exists for. The org's content lives on
/// ACME's server; the two laptops move it between themselves without that
/// server taking part in the transfer at all.
// t[verify files.topology.multi-server]
#[tokio::test]
async fn two_laptops_sync_without_the_server_taking_part() {
    let s = Scenario::open().await;
    let holder = Laptop::open().await;
    let receiver = Laptop::empty_peer(holder.album).await;
    let tree = receiver.tree.clone();
    holder
        .backend
        .admit_host(receiver.host_id(), Hosting::working());

    // The server exists and holds its own project. It is not admitted to
    // either laptop and neither laptop is admitted to it, so it could not
    // serve this transfer even if asked.
    assert!(
        s.orgs.acme.backend.admits(&holder.host_id()).is_none(),
        "fixture: the server must not be part of this"
    );

    let peer = receiver.dial_replica(&holder.endpoint).await;
    files_sync::reconcile(&receiver.backend, &peer, holder.album.get())
        .await
        .expect("laptop-to-laptop pull");

    assert!(
        tree.join("Sessions").join("Audio Files").join("vox.wav").exists(),
        "the take did not arrive"
    );
}
