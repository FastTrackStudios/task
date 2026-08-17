//! Where lanes meet.
//!
//! Each lane is tested against its own trait, which is necessary and not
//! sufficient: the interesting failures live in the seams, where one
//! lane hands a caller an id and another lane is asked to resolve it.
//!
//! This file exists because exactly that broke. `VersionService` and
//! `CurationService` were implemented in parallel and independently
//! invented incompatible readings of `VersionId` — one a `UUIDv5` hash of
//! the commit hex, the other its leading 128 bits. Both lanes' own tests
//! passed. A `VersionId` from `chain()` simply did not resolve in
//! `name_version()`, and nothing anywhere would have said so.
//!
//! `VersionId::from_commit_hex` is now the single definition. These tests
//! are what stops it quietly becoming two again.

use files::FilesBackend;
use files_proto::id::{RootId, VersionId};
use files_proto::model::RootFlavor;
use files_proto::service::curation::CurationService;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::tree::TreeService;
use files_proto::service::version::VersionService;
use files_proto::{FilesFault, RootPath};

struct Rig {
    _tmp: tempfile::TempDir,
    backend: FilesBackend,
    root: RootId,
}

async fn rig() -> Rig {
    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Mix Session".into(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");

    Rig {
        _tmp: tmp,
        backend,
        root: RootId::new(root.id),
    }
}

/// The seam that was broken: an id minted by the version lane, spent on
/// the curation lane.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_id_from_the_chain_can_be_named() {
    let rig = rig().await;
    rig.backend
        .checkpoint(rig.root, Some("first".into()))
        .await
        .expect("checkpoint");

    let chain = rig
        .backend
        .chain(rig.root, RootPath::parse("mix.wav").unwrap())
        .await
        .expect("chain");
    assert!(!chain.is_empty(), "a checkpointed file has a chain");

    // The only id a caller has is the one the chain entry carries.
    let version = VersionId::from_commit_hex(&chain[0].commit_id);

    let named = rig
        .backend
        .name_version(rig.root, version, "Approved Mix".into())
        .await
        .expect("the curation lane must resolve an id the version lane minted");
    assert_eq!(named.name, "Approved Mix");
}

/// And back the other way: a name resolves to an id the version lane
/// accepts.
#[tokio::test(flavor = "multi_thread")]
async fn a_named_version_resolves_back_onto_the_chain() {
    let rig = rig().await;
    rig.backend
        .checkpoint(rig.root, Some("first".into()))
        .await
        .expect("checkpoint");
    let chain = rig
        .backend
        .chain(rig.root, RootPath::parse("mix.wav").unwrap())
        .await
        .expect("chain");
    let version = VersionId::from_commit_hex(&chain[0].commit_id);

    rig.backend
        .name_version(rig.root, version, "Approved Mix".into())
        .await
        .expect("name");

    let resolved = rig
        .backend
        .resolve_name(rig.root, "Approved Mix".into())
        .await
        .expect("resolve");

    assert!(
        chain[0]
            .commit_id
            .starts_with(&VersionId::from_commit_hex(&resolved.commit_id).commit_prefix())
            || resolved.commit_id.starts_with(&version.commit_prefix()),
        "a name must point back at the commit it was given: \
         chain={} resolved={}",
        chain[0].commit_id,
        resolved.commit_id
    );
}

/// The roots lane mints a `RootId`; every other lane must reject an
/// unknown one the same way, rather than each inventing its own prose.
#[tokio::test(flavor = "multi_thread")]
async fn every_lane_rejects_an_unknown_root_identically() {
    let rig = rig().await;
    let ghost = RootId::generate();

    let from_roots = rig.backend.get(ghost).await.expect_err("roots");
    let from_version = rig
        .backend
        .chain(ghost, RootPath::parse("mix.wav").unwrap())
        .await
        .expect_err("version");
    let from_curation = rig
        .backend
        .named_versions(ghost, None)
        .await
        .expect_err("curation");
    let from_tree = rig
        .backend
        .browse(ghost, RootPath::root())
        .await
        .expect_err("tree");

    for (lane, fault) in [
        ("roots", from_roots),
        ("version", from_version),
        ("curation", from_curation),
        ("tree", from_tree),
    ] {
        assert!(
            matches!(fault, FilesFault::RootNotFound(id) if id == ghost),
            "{lane} must answer RootNotFound carrying the id, not prose"
        );
    }
}

/// A root released through the roots lane is gone from every lane.
#[tokio::test(flavor = "multi_thread")]
async fn releasing_a_root_closes_every_lane_over_it() {
    let rig = rig().await;
    rig.backend.release(rig.root).await.expect("release");

    assert!(matches!(
        rig.backend
            .chain(rig.root, RootPath::parse("mix.wav").unwrap())
            .await
            .expect_err("version lane must not serve a released root"),
        FilesFault::RootNotFound(_)
    ));
    assert!(matches!(
        rig.backend
            .browse(rig.root, RootPath::root())
            .await
            .expect_err("tree lane must not serve a released root"),
        FilesFault::RootNotFound(_)
    ));
}

/// The tree lane's `RootPath` and the version lane's are the same type,
/// so a path that one confines the other confines too.
#[test]
fn confinement_is_one_rule_across_lanes() {
    assert!(RootPath::parse("../../etc/passwd").is_err());
    assert!(RootPath::parse("stems/../../../etc").is_err());
    assert!(RootPath::parse("stems/kick.wav").is_ok());
}

/// A write must be visible to the tree lane immediately.
///
/// The catalogue is a separate structure that only `note_write` updates.
/// Before it existed, a file created through `WriteService` stayed
/// invisible to `entry`/`catalogue`/`changes_since` until the process
/// restarted — a catalogue that is *stale* is allowed and reported by
/// `freshness`, one that is *wrong* is not.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_is_visible_to_the_tree_lane_at_once() {
    use files_proto::service::write::WriteService;

    let rig = rig().await;

    // Make the catalogue resident first — the bug only bites a root
    // somebody has already browsed.
    let before = rig
        .backend
        .catalogue(rig.root, None)
        .await
        .expect("catalogue");
    let cursor = before.cursor.clone();

    let made = RootPath::parse("Renders").unwrap();
    rig.backend
        .create_dirs(rig.root, vec![made.clone()])
        .await
        .expect("mkdir");

    let entry = rig
        .backend
        .entry(rig.root, made.clone())
        .await
        .expect("a directory created through the write lane must be in the catalogue");
    assert_eq!(entry.path, made);

    // And it arrives as a delta rather than forcing a re-list, which is
    // what keeping the log (instead of dropping the catalogue) buys.
    let delta = rig
        .backend
        .changes_since(rig.root, cursor)
        .await
        .expect("changes_since");
    assert!(
        delta.changed.iter().any(|e| e.path == made),
        "the write should arrive as a change, not require a re-list"
    );
}

/// A delete is visible too — and as a removal, not a stale entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_removes_the_entry_rather_than_leaving_it() {
    use files_proto::service::write::WriteService;

    let rig = rig().await;
    let doomed = RootPath::parse("mix.wav").unwrap();
    rig.backend
        .catalogue(rig.root, None)
        .await
        .expect("catalogue");
    rig.backend
        .entry(rig.root, doomed.clone())
        .await
        .expect("present to begin with");

    rig.backend
        .delete_paths(rig.root, vec![doomed.clone()])
        .await
        .expect("delete");

    assert!(
        rig.backend.entry(rig.root, doomed).await.is_err(),
        "a deleted path must leave the catalogue, not linger as a stale entry"
    );
}
