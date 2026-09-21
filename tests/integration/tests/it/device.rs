//! Chapter four — a laptop that syncs the audio and leaves the footage.
//!
//! `files.sync.selective`: a device subscribes to a project type's
//! *facets*, not to path globs, so what it carries is a statement about
//! the kind of work it does and stays true when a folder is renamed.
//!
//! Two of these tests exist because the obvious assertion is not the
//! honest one — a stub must still report its real size, and the
//! catalogue saying "stub" proves nothing if the bytes are still on the
//! disk.
//!
//! # Why this chapter does not go over the wire
//!
//! Every other chapter drives a client against a server, because the
//! question is whether a person can reach something. Here the laptop
//! *is* the system under test: what is being asserted is which bytes are
//! on its disk, and its own backend is the thing deciding that. Routing
//! the calls through a server would put a network between the test and
//! its subject without changing what is being claimed.
//!
//! What that leaves untested is a device talking to an org server at
//! all — enrolment, and pulling content it does not have. That belongs
//! with `files-daemon` and is not written yet.

use files::path::RootPath;
use files::service::sync::{FacetName, SyncService};
use files::service::tree::TreeService;

use integration::device::Laptop;

/// Subscribe the laptop to the audio work and nothing else.
async fn take_the_sessions(laptop: &Laptop) {
    laptop
        .backend
        .subscribe(laptop.album, vec![FacetName("sessions".into())])
        .await
        .expect("subscribe to the sessions");
}

/// Nobody told it what these folders are.
///
/// `files.facet.tool-layout` reads the layout the tools themselves
/// impose — `Audio Files` is a REAPER session's media, `Proxies` is a
/// Resolve proxy directory — so the vocabulary is there before anyone
/// configures anything.
// t[verify files.facet.tool-layout]
// t[verify scenario.album.facets] — the mix engineer and the video editor
// subscribe to different halves, and neither configured the tool
// directories: the vocabulary is there before anyone says anything
#[tokio::test]
async fn the_tools_own_layout_names_the_facets() {
    let laptop = Laptop::open().await;
    let found = laptop.backend.facets(laptop.album).await.expect("facets");

    let named: Vec<String> = found
        .iter()
        .filter_map(|b| b.facet.as_ref().map(|f| format!("{}={}", b.path, f.0)))
        .collect();
    assert!(
        !named.is_empty(),
        "no facet was recognised from the tools' own layout"
    );
}

/// The subscription decides what is resident.
// t[verify files.sync.selective]
// t[verify scenario.album.offline] — an engineer works the album on a
// plane: the tree browses whole, and what was not asked for is visibly a
// stub rather than absent
// t[verify files.catalogue.offline]
#[tokio::test]
async fn what_the_device_did_not_ask_for_becomes_a_stub() {
    let laptop = Laptop::open().await;
    take_the_sessions(&laptop).await;

    let audio = laptop
        .backend
        .browse(
            laptop.album,
            RootPath::parse("Sessions/Audio Files").unwrap(),
        )
        .await
        .expect("browse the sessions");
    assert!(
        audio.iter().all(|e| !e.stub),
        "the work this device subscribed to was not carried"
    );

    let footage = laptop
        .backend
        .browse(laptop.album, RootPath::parse("Footage/Proxies").unwrap())
        .await
        .expect("browse the footage");
    let reel = footage
        .iter()
        .find(|e| e.name == "reel.mov")
        .expect("the footage vanished rather than becoming a stub");
    assert!(
        reel.stub,
        "the footage stayed resident on a device that did not ask for it"
    );
}

/// A placeholder that lies about size makes "how big is this project"
/// wrong on every device that did not sync it.
// t[verify files.sync.selective]
#[tokio::test]
async fn a_stub_keeps_its_name_and_its_real_size() {
    let laptop = Laptop::open().await;
    take_the_sessions(&laptop).await;

    let footage = laptop
        .backend
        .browse(laptop.album, RootPath::parse("Footage/Proxies").unwrap())
        .await
        .expect("browse the footage");
    let reel = footage.iter().find(|e| e.name == "reel.mov").expect("reel");
    assert_eq!(reel.size, Some(Laptop::REEL_BYTES));
}

/// The disk is the check that matters.
///
/// A stub that still costs 64 KB has saved nothing, whatever the
/// catalogue says about it — which is why this reads the filesystem
/// rather than asking the lane again.
// t[verify files.sync.selective]
#[tokio::test]
async fn a_stub_costs_almost_nothing_on_the_device() {
    let laptop = Laptop::open().await;
    take_the_sessions(&laptop).await;

    let on_disk = laptop.reel_on_disk();
    assert!(
        on_disk < Laptop::REEL_BYTES,
        "the file still occupies {on_disk} bytes"
    );
}

/// Unsubscribed does not mean unreachable — it means not carried.
// t[verify files.sync.selective]
#[tokio::test]
async fn opening_a_stub_brings_the_file_back_whole() {
    let laptop = Laptop::open().await;
    take_the_sessions(&laptop).await;

    laptop
        .backend
        .hydrate(
            laptop.album,
            vec![RootPath::parse("Footage/Proxies/reel.mov").unwrap()],
            true,
        )
        .await
        .expect("hydrate the reel");

    assert_eq!(laptop.reel_on_disk(), Laptop::REEL_BYTES);
}
