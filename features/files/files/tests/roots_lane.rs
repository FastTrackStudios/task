//! `RootsService` — the first v2 lane, against a real `FilesBackend`.
//!
//! In-process rather than over `LocalServer`: the lane has no
//! `permits.rs` rows yet, so it is not mounted on a router and cannot be
//! reached over vox. That is deliberate — the gate fails closed in
//! production, and a lane is granted in the same change that mounts it.
//! These call the trait directly, which is what the dispatcher would do.

use files::FilesBackend;
use files_proto::id::RootId;
use files_proto::model::RootFlavor;
use files_proto::service::roots::{AdoptRequest, AdoptionPhase, RootsService};
use files_proto::{FilesFault, RootPath};

/// A backend whose confinement boundary is its own data dir, with one
/// staged folder ready to adopt.
fn staged(name: &str) -> (tempfile::TempDir, FilesBackend, String) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join(name);
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let path = dir.to_string_lossy().into_owned();
    (data_dir, backend, path)
}

fn request(path: String, name: &str) -> AdoptRequest {
    AdoptRequest {
        path,
        name: name.to_string(),
        flavor: RootFlavor::Media,
        hash_content: true,
    }
}

// t[verify files.adopt.in-place]
#[tokio::test(flavor = "multi_thread")]
async fn adoption_moves_nothing() {
    let (_tmp, backend, path) = staged("mix-session");
    let before: Vec<_> = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();

    let root = backend
        .adopt(request(path.clone(), "Mix Session"))
        .await
        .expect("adopt");
    assert_eq!(root.name, "Mix Session");

    let after: Vec<_> = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        before.iter().all(|b| after.contains(b)),
        "adoption must not move, copy or rename anything: {before:?} -> {after:?}"
    );
    assert!(
        std::fs::read(std::path::Path::new(&path).join("mix.wav")).unwrap() == b"take one",
        "and must not touch content"
    );
}

// t[verify files.adopt.catalogue-first]
#[tokio::test(flavor = "multi_thread")]
async fn a_root_has_an_identity_before_its_progress_is_complete() {
    let (_tmp, backend, path) = staged("album");
    let root = backend.adopt(request(path, "Album")).await.expect("adopt");
    let id = RootId::new(root.id);

    // Identity first: it is listable and gettable immediately.
    assert_eq!(backend.get(id).await.expect("get").id, root.id);
    assert!(
        backend
            .list()
            .await
            .expect("list")
            .iter()
            .any(|r| r.id == root.id)
    );

    // Tracked from the moment the root exists. Which phase it is *in* is
    // deliberately not asserted: the driver starts as soon as `adopt`
    // returns, so on a fixture this small it may already be hashing, and
    // pinning the phase here would be a test of scheduling rather than
    // of the lane. `an_adoption_finishes_and_says_so` covers the end of
    // the run; this covers that there is a run to ask about.
    let progress = backend.adoption_progress(id).await.expect("progress");
    assert_eq!(progress.root_id, id);
    assert_ne!(
        progress.phase,
        AdoptionPhase::Paused,
        "adoption is tracked from the moment the root exists"
    );
}

/// The tail of `files.adopt.catalogue-first`: the addresses do get
/// computed, and the entries published without them get them.
// t[verify files.adopt.catalogue-first]
#[tokio::test(flavor = "multi_thread")]
async fn an_adoption_finishes_and_says_so() {
    let (_tmp, backend, path) = staged("album");
    let root = backend.adopt(request(path, "Album")).await.expect("adopt");
    let id = RootId::new(root.id);

    let done = backend.settled(id).await.expect("adoption progress");
    assert_eq!(done.phase, AdoptionPhase::Complete);
    assert_eq!(
        done.entries_total,
        Some(2),
        "two files: mix.wav and stems/kick.wav"
    );
    assert_eq!(
        done.entries_hashed, 2,
        "every file the walk published got an address"
    );
    assert!(done.bytes_hashed > 0);
}

/// A survey reads no bytes.
///
/// `hash_content: false` is for a tree being looked at rather than taken
/// on, and it has to stop after the walk — otherwise the cheap option
/// costs the same as the expensive one and there is no point offering
/// it.
// t[verify files.adopt.catalogue-first]
#[tokio::test(flavor = "multi_thread")]
async fn a_survey_publishes_structure_and_reads_no_content() {
    let (_tmp, backend, path) = staged("album");
    let root = backend
        .adopt(AdoptRequest {
            hash_content: false,
            ..request(path, "Album")
        })
        .await
        .expect("adopt");
    let id = RootId::new(root.id);

    let done = backend.settled(id).await.expect("adoption progress");
    assert_eq!(done.phase, AdoptionPhase::Complete);
    assert_eq!(done.entries_seen, 2, "the structure is published");
    assert_eq!(done.entries_hashed, 0, "and nothing was read");
}

// t[verify files.adopt.resumable]
#[tokio::test(flavor = "multi_thread")]
async fn adoption_pauses_and_resumes() {
    let (_tmp, backend, path) = staged("stems");
    let root = backend.adopt(request(path, "Stems")).await.expect("adopt");
    let id = RootId::new(root.id);

    let paused = backend.pause_adoption(id).await.expect("pause");
    assert_eq!(paused.phase, AdoptionPhase::Paused);

    let resumed = backend.resume_adoption(id).await.expect("resume");
    assert_eq!(
        resumed.phase,
        AdoptionPhase::Enumerating,
        "resuming continues the walk rather than restarting the adoption"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_changes_the_name_and_nothing_else() {
    let (_tmp, backend, path) = staged("takes");
    let root = backend.adopt(request(path.clone(), "Takes")).await.unwrap();
    let id = RootId::new(root.id);

    let renamed = backend
        .rename_root(id, "Live Takes".into())
        .await
        .expect("rename");
    assert_eq!(renamed.name, "Live Takes");
    assert_eq!(renamed.id, root.id, "identity survives a rename");
    assert_eq!(renamed.path, root.path, "and so does its path");

    assert_eq!(backend.get(id).await.unwrap().name, "Live Takes");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_name_is_refused() {
    let (_tmp, backend, path) = staged("takes");
    let root = backend.adopt(request(path, "Takes")).await.unwrap();
    let err = backend
        .rename_root(RootId::new(root.id), "  ".into())
        .await
        .expect_err("an empty name is not a name");
    assert!(matches!(err, FilesFault::Invalid(_)));
}

/// Releasing is not deleting — the bytes stay, and so does the history
/// that lives in the tree.
#[tokio::test(flavor = "multi_thread")]
async fn releasing_stops_tracking_and_keeps_the_bytes() {
    let (_tmp, backend, path) = staged("archive");
    let root = backend
        .adopt(request(path.clone(), "Archive"))
        .await
        .unwrap();
    let id = RootId::new(root.id);

    backend.release(id).await.expect("release");

    assert!(
        backend
            .list()
            .await
            .unwrap()
            .iter()
            .all(|r| r.id != root.id),
        "released roots are no longer tracked"
    );
    assert!(matches!(
        backend.get(id).await.expect_err("gone"),
        FilesFault::RootNotFound(_)
    ));
    assert!(
        std::path::Path::new(&path).join("mix.wav").exists(),
        "and every byte is still exactly where it was"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_root_is_a_typed_fault() {
    let (_tmp, backend, _) = staged("whatever");
    let ghost = RootId::generate();
    // The v1 surface answered `NotFound(String)` here, which a caller had
    // to parse. The variant now carries the id it could not find.
    match backend.get(ghost).await.expect_err("no such root") {
        FilesFault::RootNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_path_that_is_not_a_directory_is_refused() {
    let (tmp, backend, _) = staged("real");
    let file = tmp.path().join("notes.txt");
    std::fs::write(&file, b"not a folder").unwrap();

    assert!(
        backend
            .adopt(request(file.to_string_lossy().into_owned(), "Notes"))
            .await
            .is_err(),
        "a file is not adoptable as a root"
    );
}

/// Confinement is structural: the org boundary is the backend's, never a
/// caller-supplied argument.
#[tokio::test(flavor = "multi_thread")]
async fn adoption_cannot_reach_outside_the_org() {
    let (_tmp, backend, _) = staged("inside");
    let outside = tempfile::tempdir().expect("outside tempdir");
    std::fs::write(outside.path().join("secrets.txt"), b"nope").unwrap();

    assert!(
        backend
            .adopt(request(
                outside.path().to_string_lossy().into_owned(),
                "Elsewhere"
            ))
            .await
            .is_err(),
        "a root may not be adopted outside the org's own files area"
    );
}

#[test]
fn paths_reject_traversal_before_any_backend_sees_them() {
    // The typed path is the single place the check lives now, rather
    // than once per method.
    assert!(RootPath::parse("../../etc/passwd").is_err());
    assert!(RootPath::parse("stems/kick.wav").is_ok());
}
