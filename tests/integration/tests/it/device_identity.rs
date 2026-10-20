//! Chapter — whose subscription is it?
//!
//! `files.sync.selective` says a device subscribes to facets, and
//! `files.device.control` says what it chose survives a restart and that
//! one device can be revoked without touching the others. Both are
//! claims about *a device*, which makes the first question which device
//! a call came from.
//!
//! The answer used to be "this process". Every caller of the sync lane
//! on a host shared one subscription, a host restart made every device a
//! stranger, and revoking "the device" meant revoking the process. Now
//! the answer comes from the connection: a machine dials the org's host
//! over iroh, the handshake proves its endpoint, and the lane looks that
//! endpoint up among the devices a member enrolled.
//!
//! Everything here goes over the wire — two device endpoints dialling
//! ACME's server, an owner enrolling and revoking through her own
//! session — because the identity under test exists only on a
//! connection. The same holds between two devices with no host at all;
//! that stage is in `peer_to_peer.rs`, the chapter about having no host.

use files::id::DeviceId;
use files::service::sync::{FacetName, Subscription};

use integration::device::PeerDevice;
use integration::scenario::Scenario;

fn facets(names: &[&str]) -> Vec<FacetName> {
    names.iter().map(|n| FacetName((*n).into())).collect()
}

/// What `device` reads back as its own subscription, over its own
/// connection.
async fn own(device: &PeerDevice, host: iroh::EndpointId, root: files::RootId) -> Subscription {
    device
        .files_sync(host)
        .await
        .subscription(root)
        .await
        .expect("a device reads its own subscription")
}

/// Two machines, one root, two different answers — and they stay two
/// answers through a restart and through revoking one of them.
// t[verify files.device.identity] — the device is the endpoint the
// connection proved, enrolled by a member; a restart on the same key is
// the same device
// t[verify files.device.control] — revocation cuts one device and only
// that one
// t[verify files.sync.selective] — each device's facets are its own
#[tokio::test]
async fn two_devices_keep_disjoint_subscriptions_through_restart_and_revocation() {
    let s = Scenario::open().await;
    let host = s.orgs.acme.endpoint.id();
    let root = s.acme_root;

    let mix_room = PeerDevice::new().await;
    let mut edit_bay = PeerDevice::new().await;

    // Paired the way the app pairs (`device_pairing`): a member, signed
    // in, hands the org each machine's endpoint id.
    let alice = s.as_alice().await.devices().await;
    let mix_row = alice
        .enroll_device(mix_room.id(), "mix room".into())
        .await
        .expect("enrol the mix room");
    let edit_row = alice
        .enroll_device(edit_bay.id(), "edit bay".into())
        .await
        .expect("enrol the edit bay");
    assert_ne!(mix_row.id, edit_row.id);

    // Each sets its own, as itself.
    mix_room
        .files_sync(host)
        .await
        .subscribe(root, facets(&["sessions"]))
        .await
        .expect("the mix room subscribes");
    edit_bay
        .files_sync(host)
        .await
        .subscribe(root, facets(&["proxies", "mixes"]))
        .await
        .expect("the edit bay subscribes");

    let mix = own(&mix_room, host, root).await;
    let edit = own(&edit_bay, host, root).await;
    assert_eq!(
        mix.device, mix_row.id,
        "the mix room read someone else's row"
    );
    assert_eq!(
        edit.device, edit_row.id,
        "the edit bay read someone else's row"
    );
    assert_eq!(mix.facets, facets(&["sessions"]));
    assert_eq!(
        edit.facets,
        facets(&["mixes", "proxies"]),
        "one device's subscription overwrote another's"
    );

    // The host's own node is a third device, and neither machine's
    // choice landed on it. Before, all three were one row.
    let node = alice.subscription(root).await.expect("the node's own");
    assert!(
        node.facets.is_empty(),
        "a device's subscription was applied to the host itself: {node:?}"
    );
    assert!(node.device != mix_row.id && node.device != edit_row.id);

    // Restart one. Same key, fresh addresses — it is the same device.
    edit_bay.restart().await;
    let edit = own(&edit_bay, host, root).await;
    assert_eq!(
        edit.device, edit_row.id,
        "a restart made the edit bay a stranger"
    );
    assert_eq!(edit.facets, facets(&["mixes", "proxies"]));
    assert_eq!(
        own(&mix_room, host, root).await.facets,
        facets(&["sessions"])
    );

    // One row per machine, however many times it has connected.
    let rows = alice.devices().await.expect("devices");
    for (id, name) in [(mix_row.id, "mix room"), (edit_row.id, "edit bay")] {
        assert_eq!(
            rows.iter().filter(|d| d.id == id).count(),
            1,
            "{name} is not exactly one device: {rows:?}"
        );
    }

    // Revoke the mix room. It is refused on its next call, on a fresh
    // connection or not.
    alice
        .revoke_device(mix_row.id)
        .await
        .expect("revoke the mix room");
    let refused = mix_room.files_sync(host).await.subscription(root).await;
    assert!(
        refused.is_err(),
        "a revoked device still read its subscription: {refused:?}"
    );
    let refused = mix_room
        .files_sync(host)
        .await
        .subscribe(root, facets(&["proxies"]))
        .await;
    assert!(
        refused.is_err(),
        "a revoked device still changed its subscription: {refused:?}"
    );

    // The other device is untouched.
    let edit = own(&edit_bay, host, root).await;
    assert_eq!(edit.device, edit_row.id);
    assert_eq!(
        edit.facets,
        facets(&["mixes", "proxies"]),
        "revoking one device disturbed another"
    );

    // And the revoked device's record is kept, not deleted: revocation
    // is a state, so the machine cannot reconnect as a stranger.
    let kept = s
        .orgs
        .acme
        .backend
        .device_subscription(mix_row.id, root)
        .expect("the revoked device's record");
    assert_eq!(kept.facets, facets(&["sessions"]));
}

/// A machine nobody enrolled is not a device of this org, whatever it can
/// prove about its own key.
// t[verify files.device.identity]
#[tokio::test]
async fn an_unenrolled_machine_has_no_subscription_to_read_or_write() {
    let s = Scenario::open().await;
    let stranger = PeerDevice::new().await;
    let lane = stranger.files_sync(s.orgs.acme.endpoint.id()).await;

    assert!(lane.subscription(s.acme_root).await.is_err());
    assert!(
        lane.subscribe(s.acme_root, facets(&["sessions"]))
            .await
            .is_err(),
        "an unenrolled endpoint recorded a subscription"
    );
}

/// An enrolled device may say what it keeps, and that is all it may do
/// on this lane: it cannot list, enrol or revoke the org's devices.
// t[verify files.peering.scope]
#[tokio::test]
async fn an_enrolled_device_cannot_administer_the_orgs_devices() {
    let s = Scenario::open().await;
    let host = s.orgs.acme.endpoint.id();
    let device = PeerDevice::new().await;
    let other = PeerDevice::new().await;
    let alice = s.as_alice().await.devices().await;
    alice
        .enroll_device(device.id(), "laptop".into())
        .await
        .expect("enrol");
    let other_row = alice
        .enroll_device(other.id(), "other".into())
        .await
        .expect("enrol the other");

    let lane = device.files_sync(host).await;
    lane.subscription(s.acme_root)
        .await
        .expect("fixture: an enrolled device reaches its own subscription");
    assert!(
        lane.devices().await.is_err(),
        "a device listed the org's devices"
    );
    assert!(
        lane.revoke_device(other_row.id).await.is_err(),
        "a device revoked another device"
    );
    assert!(
        lane.enroll_device(PeerDevice::new().await.id(), "rogue".into())
            .await
            .is_err(),
        "a device enrolled a machine"
    );
}

/// The host restarting changes nothing about whose subscription is whose,
/// and a revocation issued before it is still in force after it.
///
/// The half `this_device()` could never have passed: its id was minted
/// per process, so a restarted host read every device as a stranger and
/// a revocation lasted exactly as long as the process that issued it.
// t[verify files.device.identity]
// t[verify files.device.control]
#[tokio::test]
async fn ownership_and_revocation_survive_the_host_restarting() {
    let s = Scenario::open().await;
    let root = s.acme_root;
    let mix_room = PeerDevice::new().await;
    let edit_bay = PeerDevice::new().await;

    let (mix_row, edit_row, node_before): (DeviceId, DeviceId, DeviceId) = {
        let alice = s.as_alice().await.devices().await;
        let mix = alice
            .enroll_device(mix_room.id(), "mix room".into())
            .await
            .unwrap();
        let edit = alice
            .enroll_device(edit_bay.id(), "edit bay".into())
            .await
            .unwrap();
        let node = alice.subscription(root).await.unwrap().device;
        (mix.id, edit.id, node)
    };
    let host = s.orgs.acme.endpoint.id();
    mix_room
        .files_sync(host)
        .await
        .subscribe(root, facets(&["sessions"]))
        .await
        .unwrap();
    edit_bay
        .files_sync(host)
        .await
        .subscribe(root, facets(&["proxies"]))
        .await
        .unwrap();
    s.as_alice()
        .await
        .devices()
        .await
        .revoke_device(mix_row)
        .await
        .expect("revoke before the restart");

    // On disk, which is the only thing a restart can honestly be about:
    // both devices' rows and the revocation, under the org's own data.
    let on_disk = std::fs::read_to_string(s.orgs.acme.backend.data_dir().join("sync.json"))
        .expect("the sync lane's state is on the org's disk");
    for id in [mix_row, edit_row, node_before] {
        assert!(
            on_disk.contains(&id.to_string()),
            "{id} is not on disk: {on_disk}"
        );
    }

    let Scenario { orgs, people, .. } = s;
    let acme = orgs.acme.restart().await;
    assert_eq!(acme.endpoint.id(), host, "fixture: the host kept its key");

    let edit = own(&edit_bay, host, root).await;
    assert_eq!(edit.device, edit_row, "the host forgot who the edit bay is");
    assert_eq!(edit.facets, facets(&["proxies"]));
    assert!(
        mix_room
            .files_sync(host)
            .await
            .subscription(root)
            .await
            .is_err(),
        "a revocation did not outlive the host's restart"
    );

    // And the host's own node is the device it was.
    let alice = integration::client::Session::open(&acme, people.alice.token.clone())
        .await
        .devices()
        .await;
    assert_eq!(
        alice.subscription(root).await.unwrap().device,
        node_before,
        "the host came back as a different device"
    );
}
