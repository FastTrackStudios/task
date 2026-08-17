//! Chapter one — the tree is already there, and becomes ours.
//!
//! Adoption, the ignore layers, one transactional write, a checkpoint,
//! and the two ways bytes leave: a ticket and an archive. Everything
//! here happens inside one org on one server; the collaboration starts
//! in `collaboration.rs`.

use files::path::RootPath;
use files::service::media::MediaService;
use files::service::roots::AdoptionPhase;
use files::service::tree::TreeService;
use files::service::version::VersionService;
use files::service::write::WriteService;

use integration::scenario::Scenario;

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

// t[verify files.adopt.catalogue-first]
#[tokio::test]
async fn a_tree_is_browsable_the_moment_it_is_adopted() {
    let s = Scenario::open().await;
    // Not "after hashing finishes": structure is published first and
    // content addresses are computed behind it, because a 244 GB
    // project that is invisible until it is hashed is a project nobody
    // can work on that day.
    let listing = s
        .orgs
        .acme
        .backend
        .browse(s.acme_root, RootPath::root())
        .await
        .expect("browse");
    let names: Vec<&str> = listing.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Audio Files"), "{names:?}");
    assert!(names.contains(&"Deliverables"), "{names:?}");
    assert!(names.contains(&"Song.rpp"), "{names:?}");
}

// t[verify files.adopt.in-place]
#[tokio::test]
async fn adoption_moves_nothing() {
    let s = Scenario::open().await;
    // The files are still where their applications put them. This is
    // the whole promise of adoption: REAPER keeps writing the same
    // paths it was writing before, during and after.
    let session = s.orgs.acme.tree().join("Song");
    assert_eq!(
        std::fs::read(session.join("Audio Files").join("kick.wav")).unwrap(),
        b"kick take one"
    );
    assert!(session.join("Song.rpp").exists());
}

// t[verify files.ignore.layers]
#[tokio::test]
async fn platform_junk_never_surfaces() {
    let s = Scenario::open().await;
    let root = s
        .orgs
        .acme
        .backend
        .browse(s.acme_root, RootPath::root())
        .await
        .expect("browse");
    let audio = s
        .orgs
        .acme
        .backend
        .browse(s.acme_root, p("Audio Files"))
        .await
        .expect("browse");

    assert!(
        !root.iter().any(|e| e.name == ".DS_Store"),
        "Finder droppings reached a listing"
    );
    assert!(
        !audio.iter().any(|e| e.name.starts_with("._")),
        "an AppleDouble sidecar reached a listing"
    );
}

// t[verify files.write.surface]
#[tokio::test]
async fn a_write_is_one_operation_and_reaches_the_catalogue() {
    let s = Scenario::open().await;
    let receipt = s
        .orgs
        .acme
        .backend
        .create_dirs(s.acme_root, vec![p("Renders")])
        .await
        .expect("mkdir");
    assert!(!receipt.operation.is_empty(), "a write with no operation id");

    // Without a restart, and as a delta rather than a re-listing —
    // `files.catalogue.concurrent`. A catalogue that only hears about
    // writes at startup is a catalogue that is wrong all day.
    s.orgs
        .acme
        .backend
        .entry(s.acme_root, p("Renders"))
        .await
        .expect("the catalogue did not hear about the write");
}

// t[verify files.version.cadence]
#[tokio::test]
async fn a_checkpoint_records_history() {
    let s = Scenario::open().await;
    let checkpoint = s
        .orgs
        .acme
        .backend
        .checkpoint(s.acme_root, Some("first".into()))
        .await
        .expect("checkpoint");
    assert!(!checkpoint.commit_id.is_empty());
}

/// Bytes leave by ticket, and adoption is what makes them leavable.
///
/// The byte lane reads the pinned head rather than the disk — a file
/// being written to right now has no stable length and no stable
/// content, so a ticket for it could only promise what it cannot keep.
/// Adoption's second pass is what supplies that head: it reads the tree
/// into the store and pins what it read, so an adopted project is
/// streamable without anyone having checkpointed it by hand.
// t[verify files.scale.large-media]
#[tokio::test]
async fn a_read_mints_a_ticket_rather_than_returning_bytes() {
    let s = Scenario::open().await;
    let ticket = s
        .orgs
        .acme
        .backend
        .read(s.acme_root, p("Audio Files/kick.wav"))
        .await
        .expect("a byte ticket");

    // The size is known because it was read from the store, and the
    // ticket is seekable because both stores read by range. Neither is
    // aspirational — the archive below reports the opposite, honestly.
    assert_eq!(ticket.length, Some(13));
    assert!(ticket.seekable);
}

/// Adoption finishes, and says so.
///
/// `files.adopt.catalogue-first` is two claims, and this is the second:
/// the addresses do get computed. The first — that the tree is browsable
/// before they are — is the test at the top of this file, which runs
/// against the same fixture and does not wait for anything.
// t[verify files.adopt.catalogue-first]
#[tokio::test]
async fn adoption_hashes_the_tree_behind_the_catalogue() {
    let s = Scenario::open().await;
    let done = s
        .orgs
        .acme
        .backend
        .settled(s.acme_root)
        .await
        .expect("adoption progress");

    assert_eq!(done.phase, AdoptionPhase::Complete);
    // Four files: the project, two takes, one deliverable. The junk is
    // not among them — `files.ignore.layers` applies to what adoption
    // publishes, not only to what a browse renders.
    assert_eq!(done.entries_total, Some(4), "{done:?}");
    assert_eq!(done.entries_hashed, 4, "{done:?}");
    assert!(done.bytes_hashed > 0);
}

/// The catalogue's entries carry their addresses once hashing has run.
///
/// `content: None` is what an unverified entry looks like, and an
/// adoption that never wrote the addresses back would leave every entry
/// in that state forever: browsable, and permanently unable to say what
/// it holds.
// t[verify files.adopt.catalogue-first]
#[tokio::test]
async fn a_hashed_entry_knows_its_content_address() {
    let s = Scenario::open().await;
    let kick = s
        .orgs
        .acme
        .backend
        .entry(s.acme_root, p("Audio Files/kick.wav"))
        .await
        .expect("the take is catalogued");
    assert!(
        kick.content.is_some(),
        "an adopted file stayed unverified: {kick:?}"
    );
}

// t[verify files.write.surface]
#[tokio::test]
async fn an_archive_is_generated_as_it_is_sent() {
    let s = Scenario::open().await;
    let archive = s
        .orgs
        .acme
        .backend
        .archive(s.acme_root, vec![p("Audio Files")])
        .await
        .expect("an archive ticket");

    assert_eq!(archive.content_type, "application/x-tar");
    // `None`, and that is the honest answer: a tar's size is not known
    // until it has been produced, and putting a guess on the wire that
    // the body then fails to match is worse than admitting it.
    assert_eq!(archive.length, None);
    assert!(!archive.seekable, "a one-pass stream claimed to be seekable");
}
