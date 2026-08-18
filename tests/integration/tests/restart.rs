//! Chapter six — what survives the process.
//!
//! Every other chapter runs against a server that has been up since the
//! test started, which is the one condition a real server is never in.
//! This one stops a server and starts it again on the same disk, and
//! asks what is still true.
//!
//! The answers are load-bearing rather than incidental:
//!
//! - **The endpoint id.** A device is registered against an id and
//!   nothing else, so an id that changed on restart would unregister
//!   every device the server had.
//! - **The catalogue.** `files.catalogue.offline` promises a browse with
//!   the holding location absent; a cold process that rebuilds by
//!   walking cannot honour that, because walking is the thing it cannot
//!   do.
//! - **Admissions.** A restart that forgot them would silently close an
//!   org to every peer that hosts it — or, worse the other way, a
//!   restart that re-admitted a dismissed host would undo a revocation
//!   nobody re-issued.
//!
//! What deliberately does *not* survive is the auth store: it is
//! `sqlite::memory:`, so accounts and sessions go with the process.
//! That is the harness's shape, not the product's, which is why nothing
//! here asserts about people.
//!
//! # Why some of these also read the file
//!
//! A restart here is a new `AppState` in the *same* process, and the
//! durable state is cached in process-global maps keyed by data
//! directory. So a restarted server can answer a question correctly
//! having read nothing — from a cache the old process left behind.
//!
//! Which means "the restarted server answered" is a weaker claim than it
//! looks, and on its own it would pass on a build that persisted
//! nothing at all. Where the point of the test is durability rather than
//! continuity, it reads the file too. The files crate's own suite
//! learned this the hard way; the note is here so this suite does not
//! learn it again.

use files::model::RootFlavor;
use files::path::RootPath;
use files::service::roots::RootsService;
use files::service::tree::TreeService;
use files_domain::Hosting;

use integration::scenario::Scenario;

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

/// The id a device was registered against outlives the process.
// t[verify files.topology.multi-server]
// t[verify files.device.identity] — "an identity it holds and persists
// itself... surviving restart": the key is the identity, so coming back
// on the same id is the rule holding. A server rather than a laptop, but
// the mechanism is one mechanism — see `device.rs` on why a machine has
// one identity and not two
#[tokio::test]
async fn a_server_comes_back_on_the_same_endpoint_id() {
    let s = Scenario::open().await;
    let before = s.orgs.acme.host_id();

    let acme = s.orgs.acme.restart().await;

    assert_eq!(
        acme.host_id(),
        before,
        "a restart changed the id every device was registered against"
    );
}

/// The catalogue is read back rather than rebuilt.
///
/// The tree is deleted before the restart, so a server that answered by
/// walking would answer empty — and "the folder is empty" and "the disk
/// is gone" are the two states `files.catalogue.offline` exists to keep
/// apart.
// t[verify files.catalogue.offline]
#[tokio::test]
async fn a_tree_is_still_browsable_after_a_restart_without_it() {
    let s = Scenario::open().await;
    let root = s.acme_root;
    let session = s.orgs.acme.tree().join("Song");

    // Cold, and blind: the catalogue is the only thing left that knows.
    std::fs::remove_dir_all(&session).expect("remove the tree");
    let acme = s.orgs.acme.restart().await;

    let listed = acme
        .backend
        .browse(root, RootPath::root())
        .await
        .expect("a restarted server must still list a tree it cannot see");
    let mut names: Vec<&str> = listed.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert!(names.contains(&"Audio Files"), "{names:?}");
    assert!(names.contains(&"Song.rpp"), "{names:?}");

    // And it is genuinely on disk, not merely in a cache this process
    // never dropped — see the module note.
    let raw = std::fs::read_to_string(acme.backend.data_dir().join("catalogue.json"))
        .expect("the catalogue reached disk");
    assert!(raw.contains("Song.rpp"), "{raw:.400}");
}

/// And sizes come back with it, so "how big is this project" is not
/// answered as zero by a server that has lost sight of the disk.
// t[verify files.catalogue.offline]
#[tokio::test]
async fn a_restarted_server_still_knows_how_big_the_project_is() {
    let s = Scenario::open().await;
    let root = s.acme_root;
    std::fs::remove_dir_all(s.orgs.acme.tree().join("Song")).expect("remove the tree");

    let acme = s.orgs.acme.restart().await;
    let kick = acme
        .backend
        .entry(root, p("Audio Files/kick.wav"))
        .await
        .expect("the take is still known to exist");

    assert_eq!(kick.size, 13, "a stub-like entry reported the wrong size");
}

/// A root a server hosts without holding survives too — the structure
/// host's whole state is what it wrote down.
// t[verify files.peering.replication]
#[tokio::test]
async fn a_hosted_root_is_still_hosted_after_a_restart() {
    let s = Scenario::open().await;
    s.orgs
        .vnt
        .backend
        .host_structure(s.acme_root, "ACME Song".into(), RootFlavor::Media)
        .await
        .expect("host ACME's structure");

    let vnt = s.orgs.vnt.restart().await;

    let listed = RootsService::list(&vnt.backend).await.expect("list");
    let hosted = listed
        .iter()
        .find(|r| r.id == s.acme_root.get())
        .expect("the hosted root went with the process");
    assert_eq!(
        hosted.path, None,
        "a restart gave a structure-only root a tree"
    );
}

/// Admission survives, so a peer does not have to be re-admitted every
/// time the server it hosts is restarted.
// t[verify files.peering.presence]
#[tokio::test]
async fn an_admitted_host_stays_admitted_across_a_restart() {
    let s = Scenario::open().await;
    let vnt = s.orgs.vnt.host_id();
    s.orgs
        .acme
        .backend
        .admit_host(vnt.clone(), Hosting::structure_only());

    let acme = s.orgs.acme.restart().await;

    assert_eq!(
        acme.backend.admits(&vnt),
        Some(Hosting::structure_only()),
        "a restart forgot which servers may host this org"
    );

    let raw = std::fs::read_to_string(acme.backend.data_dir().join("hosts.json"))
        .expect("the admitted set reached disk");
    assert!(raw.contains(&vnt.0), "{raw}");
}

/// And so does dismissal — the direction that matters more.
///
/// A restart that forgot an admission costs a peer some downtime. One
/// that forgot a *dismissal* silently re-opens the org to a machine
/// somebody deliberately cut off, and nothing would say so.
// t[verify files.peering.presence]
#[tokio::test]
async fn a_dismissed_host_stays_dismissed_across_a_restart() {
    let s = Scenario::open().await;
    let vnt = s.orgs.vnt.host_id();
    s.orgs
        .acme
        .backend
        .admit_host(vnt.clone(), Hosting::structure_only());
    s.orgs.acme.backend.dismiss_host(&vnt);

    let acme = s.orgs.acme.restart().await;

    assert_eq!(
        acme.backend.admits(&vnt),
        None,
        "a restart re-admitted a host somebody had dismissed"
    );

    let raw = std::fs::read_to_string(acme.backend.data_dir().join("hosts.json"))
        .expect("the admitted set reached disk");
    assert!(
        !raw.contains(&vnt.0),
        "the dismissal never reached the file: {raw}"
    );
}
