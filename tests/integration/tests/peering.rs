//! Chapter five — an org on as many servers as it wants.
//!
//! Hosting an org means *running* it: knowing its structure, serving its
//! members, accepting writes. It does not mean holding its content.
//! Keeping those two separate is what makes a second host cost the size
//! of a catalogue rather than the size of a library — and it is why a
//! backup needs no special case, being simply a host nobody works on.
//!
//! The first several tests are about the model: what an org survives,
//! what a host is for, what adding one buys. The last is the one that
//! took the work — structure actually converging between the two
//! servers, over iroh, by replicating the commit graph and re-deriving
//! the catalogue locally.

use files::model::RootFlavor;
use files::path::RootPath;
use files::service::roots::RootsService;
use files::service::tree::TreeService;
use files_domain::{HostId, Hosting, OrgId, Peering};

use integration::client::Session;
use integration::scenario::Scenario;

/// The starting arrangement: both orgs known to both servers, with the
/// content of each on its own company's machine.
///
/// This is what "two companies, one project" looks like as placement.
/// VNT's server knows ACME — it can serve an ACME member — and holds not
/// one byte of ACME's takes.
fn both_orgs_on_both_servers(s: &Scenario) -> (Peering, OrgId, OrgId, HostId, HostId) {
    let acme_host = HostId(s.orgs.acme.endpoint.id().to_string());
    let vnt_host = HostId(s.orgs.vnt.endpoint.id().to_string());
    let acme_org = OrgId("acme-audio".into());
    let vnt_org = OrgId("vnt-video".into());

    let mut peering = Peering::new();
    peering
        .host(acme_org.clone(), acme_host.clone(), Hosting::working())
        .host(acme_org.clone(), vnt_host.clone(), Hosting::structure_only())
        .host(vnt_org.clone(), vnt_host.clone(), Hosting::working())
        .host(vnt_org.clone(), acme_host.clone(), Hosting::structure_only());

    (peering, acme_org, vnt_org, acme_host, vnt_host)
}

/// Presence and placement are separate questions.
// t[verify files.peering.presence]
#[tokio::test]
async fn an_org_can_be_known_by_a_server_that_holds_none_of_it() {
    let s = Scenario::open().await;
    let (peering, acme_org, ..) = both_orgs_on_both_servers(&s);

    assert_eq!(peering.hosts_of(&acme_org).count(), 2, "both servers know it");
    assert_eq!(
        peering.content_hosts(&acme_org).count(),
        1,
        "and exactly one holds its bytes"
    );
}

/// A peer sees what it hosts and nothing else.
// t[verify files.peering.scope]
#[tokio::test]
async fn a_host_sees_only_the_orgs_it_hosts() {
    let s = Scenario::open().await;
    let (peering, .., vnt_host) = both_orgs_on_both_servers(&s);
    assert_eq!(peering.orgs_on(&vnt_host).len(), 2);
    assert!(peering.orgs_on(&HostId("a-stranger".into())).is_empty());
}

/// A backup is a host nobody works on.
///
/// Note the "before" half. Without it this test would pass on a system
/// where the org already survived everything, proving nothing about the
/// backup — the state peering exists to fix has to be visible first.
// t[verify files.peering.backup]
#[tokio::test]
async fn a_backup_changes_what_an_org_survives() {
    let s = Scenario::open().await;
    let (mut peering, acme_org, _, acme_host, _) = both_orgs_on_both_servers(&s);
    assert!(
        !peering.survives_loss_of(&acme_org, &acme_host),
        "fixture: losing ACME's own server must cost ACME its content"
    );

    peering.host(
        acme_org.clone(),
        HostId("offsite-backup".into()),
        Hosting::backup(),
    );

    assert!(peering.survives_loss_of(&acme_org, &acme_host));
    assert_eq!(peering.backups(&acme_org).count(), 1);
}

/// Every host runs the org.
///
/// VNT's server holds none of ACME's bytes and is still a place an ACME
/// member can work — it fetches content from a host that has it. Nothing
/// elects a leader, so losing any host costs reach and capacity, never
/// availability.
// t[verify files.peering.serving]
#[tokio::test]
async fn a_host_that_holds_no_bytes_still_serves_the_org() {
    let s = Scenario::open().await;
    let (mut peering, acme_org, _, acme_host, vnt_host) = both_orgs_on_both_servers(&s);
    for region in ["eu-west", "ap-south"] {
        peering.host(
            acme_org.clone(),
            HostId(format!("acme-{region}")),
            Hosting::working(),
        );
    }

    assert!(
        peering.hosts_of(&acme_org).all(|(h, _)| peering.serves(h, &acme_org)),
        "a host that holds no bytes was treated as a cache"
    );
    assert!(peering.serves(&vnt_host, &acme_org));
    assert!(
        peering.available_without(&acme_org, &acme_host),
        "ACME went down with its own server"
    );
}

/// An org grows by adding servers.
///
/// A host is a storage location, so attaching a server attaches its
/// capacity — and no host has to hold everything, which is what lets an
/// org outgrow any one machine.
// t[verify files.peering.scale]
#[tokio::test]
async fn adding_a_server_adds_capacity() {
    let s = Scenario::open().await;
    let (mut peering, acme_org, ..) = both_orgs_on_both_servers(&s);
    peering.host(
        acme_org.clone(),
        HostId("offsite-backup".into()),
        Hosting::backup(),
    );
    let before = peering.content_hosts(&acme_org).count();

    for n in 1..=3 {
        peering.host(
            acme_org.clone(),
            HostId(format!("acme-shelf-{n}")),
            Hosting::capacity(),
        );
    }

    assert_eq!(peering.content_hosts(&acme_org).count(), before + 3);
    // Shares, not copies. Three more machines holding parts of the
    // library is not three more restore points, and a model that counted
    // them as such would report an org as safe that is not. The one
    // complete host is still the backup, which is the only thing here
    // that ever claimed to be a full copy.
    assert_eq!(peering.complete_hosts(&acme_org).count(), 1);
}

/// Structure converges between the two servers, live.
///
/// VNT hosts ACME's root — same id, no tree — and pulls the commit graph
/// over iroh. What arrives is the shape: what exists, where, and how
/// big. What does not arrive is a single chunk, which is the whole
/// point.
///
/// # What signing this call reveals
///
/// The pull is signed with an ACME session token, and it has to be: the
/// replica lane sits behind the same permission gate as everything else
/// and asks "is this caller a member of this org", which an anonymous
/// peer can only ever fail.
///
/// A host is not a person, so there is no host credential to present —
/// what stands in here is a user account on the org being replicated.
/// That is a real gap and this is where it shows: admitting a server to
/// host an org is a different act from hiring someone, and until it has
/// its own credential, "which hosts may pull this org" is answered by
/// who happens to hold a login.
///
/// The alternative was worse and was the state until this suite served
/// the real router: the lane mounted nowhere, so replication worked in
/// a harness and nowhere else.
// t[verify files.peering.replication]
#[tokio::test]
async fn structure_replicates_between_servers_without_moving_content() {
    let s = Scenario::open().await;

    // VNT dials ACME as an admitted host. The credential is Alice's,
    // for want of a host's own — see above.
    let admitted = Session::open(&s.orgs.acme, s.people.alice.token.clone()).await;
    let peer = admitted.replica().await;

    // The receiving side of `files.peering.replication`: the root
    // becomes real here, with the id it has everywhere, and no tree
    // underneath it.
    let hosted = s
        .orgs
        .vnt
        .backend
        .host_structure(s.acme_root, "ACME Song".into(), RootFlavor::Media)
        .await
        .expect("host ACME's structure");
    assert!(!hosted.is_placed(), "a structure host claimed a tree");

    let report = files_sync::reconcile_structure(&s.orgs.vnt.backend, &peer, s.acme_root.get())
        .await
        .expect("pull the structure");
    assert_eq!(
        report.chunks_fetched, 0,
        "structure replication moved content"
    );

    // Browsed on VNT, from a commit graph rather than a disk.
    let listed = s
        .orgs
        .vnt
        .backend
        .browse(s.acme_root, RootPath::parse("Audio Files").unwrap())
        .await
        .expect("browse the replicated structure");
    let mut names: Vec<&str> = listed.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert!(names.contains(&"vox.wav"), "{names:?}");

    // Sizes come from the manifests, so "how big is this project"
    // answers correctly on a host holding none of it.
    let bytes: u64 = listed.iter().filter_map(|e| e.size).sum();
    assert!(bytes > 0, "the structure arrived without sizes");
}

/// The gap above, stated so it cannot quietly close the wrong way.
///
/// Nothing about an unauthenticated peer should be able to walk an org's
/// commit graph: `object` returns raw commits and trees, and `chunks`
/// returns content. If someone makes the replica lane public to "get
/// peering working", this fails — which is the point of writing it down
/// as a test rather than as a comment.
// t[verify files.peering.replication]
#[tokio::test]
async fn an_unadmitted_peer_cannot_walk_the_commit_graph() {
    let s = Scenario::open().await;
    let stranger = Session::anonymous(&s.orgs.acme).await;

    let refused = stranger.replica().await.heads(s.acme_root.get()).await;
    assert!(
        refused.is_err(),
        "an anonymous peer read an org's heads: {refused:?}"
    );
}
