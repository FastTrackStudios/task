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
        tree.join("Sessions")
            .join("Audio Files")
            .join("vox.wav")
            .exists(),
        "the take did not arrive"
    );
}

/// Whose subscription is whose, with no host anywhere (ADR 0006 — peer
/// parity).
///
/// The host-side chapter (`device_identity.rs`) proves this against an
/// org's server. Here the node holding the records is a laptop, the other
/// device is a laptop, and no server was ever booted: one device's
/// choices must still be its own, survive its restart, and be revocable
/// without touching the node's own. A rule that only held because a
/// server was in the middle would be a rule about servers.
// t[verify files.device.identity]
// t[verify files.device.control]
// t[verify files.sync.selective]
#[tokio::test]
async fn two_laptops_keep_their_own_subscriptions_with_no_host() {
    use files::service::sync::{FacetName, SyncService};

    let sessions = vec![FacetName("sessions".into())];
    let proxies = vec![FacetName("proxies".into())];

    let holder = Laptop::open().await;
    let mut other = Laptop::empty_peer(holder.album).await;
    let album = holder.album;

    // Paired at the holder: somebody at that machine enrols the other
    // one's endpoint, exactly as a member would on a server.
    let other_row = holder
        .backend
        .enroll_device(other.endpoint.id().to_string(), "second laptop".into())
        .await
        .expect("enrol the second laptop");

    // The holder's own choice, made at the holder — its disk follows.
    holder
        .backend
        .subscribe(album, sessions.clone())
        .await
        .expect("the holder subscribes");
    // The other's choice, made over the wire as itself.
    let theirs = other
        .files_sync(holder.endpoint.id())
        .await
        .subscribe(album, proxies.clone())
        .await
        .expect("the second laptop subscribes on the first");
    assert_eq!(theirs.device, other_row.id);

    let mine = holder.backend.subscription(album).await.expect("own");
    assert_eq!(
        mine.facets, sessions,
        "the other laptop's choice landed on the holder"
    );
    assert_ne!(mine.device, other_row.id);
    assert!(
        holder.reel_on_disk() < Laptop::REEL_BYTES,
        "another device subscribing to proxies hydrated the holder's footage"
    );

    // The other laptop restarts — same key — and is the same device.
    other.restart().await;
    let theirs = other
        .files_sync(holder.endpoint.id())
        .await
        .subscription(album)
        .await
        .expect("after its restart");
    assert_eq!(theirs.device, other_row.id, "a restart made it a stranger");
    assert_eq!(theirs.facets, proxies);

    // A peer may say what it keeps, not administer the holder's devices.
    let lane = other.files_sync(holder.endpoint.id()).await;
    assert!(
        lane.devices().await.is_err(),
        "a peer listed the holder's devices"
    );
    assert!(
        lane.revoke_device(mine.device).await.is_err(),
        "a peer revoked the holder's own device"
    );

    // Revoke it at the holder.
    holder
        .backend
        .revoke_device(other_row.id)
        .await
        .expect("revoke the second laptop");
    let refused = other
        .files_sync(holder.endpoint.id())
        .await
        .subscription(album)
        .await;
    assert!(refused.is_err(), "a revoked laptop still read: {refused:?}");

    // The holder's own is intact, on the lane and on the disk.
    let mine = holder.backend.subscription(album).await.expect("own");
    assert_eq!(
        mine.facets, sessions,
        "revoking a peer disturbed the holder's own"
    );
    assert!(holder.reel_on_disk() < Laptop::REEL_BYTES);
    assert!(
        holder
            .tree
            .join("Sessions")
            .join("Audio Files")
            .join("vox.wav")
            .metadata()
            .is_ok_and(|m| m.len() == b"vox take one".len() as u64),
        "the holder's subscribed sessions stopped being resident"
    );
}
