//! A host that knows a root and does not hold it — `files.peering.replication`.
//!
//! Identity and placement are separate: `id` is the root, the same on
//! every host that knows it, and `path` is only where its tree happens
//! to sit *here*. A host with no copy of the tree is not a broken host,
//! it is the cheap one — the reason a second host costs a catalogue
//! rather than a library.
//!
//! What must hold on such a host:
//!
//! - structure questions answer, from the catalogue
//! - content questions refuse, and say why
//! - nothing resolves a path against the process's working directory
//!
//! That last one is why `path` is an `Option` rather than an empty
//! string. `Path::new("").join("Stems")` is *relative*, so a single
//! missed call site would read or write files belonging to whatever
//! directory the server happened to start in.

use files::{FilesBackend, FilesService, RootFlavor};
use files_proto::id::RootId;
use files_proto::path::RootPath;
use files_proto::service::roots::RootsService;
use files_proto::service::tree::TreeService;
use files_proto::service::write::WriteService;

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

async fn host() -> (tempfile::TempDir, FilesBackend, RootId) {
    let data = tempfile::tempdir().expect("data dir");
    let backend = FilesBackend::new(data.path(), data.path().join("vault")).expect("backend");
    // The id is the one the root has on the host that holds its tree.
    // Minting a fresh one here would make the same root two roots.
    let id = RootId::new(uuid::Uuid::from_bytes([9; 16]));
    let root = backend
        .host_structure(id, "Album".into(), RootFlavor::Media)
        .await
        .expect("host an org's structure");
    assert!(!root.is_placed());
    (data, backend, id)
}

// t[verify files.peering.replication]
#[tokio::test]
async fn a_root_can_be_hosted_without_being_placed() {
    let (_data, backend, id) = host().await;
    let listed = RootsService::list(&backend).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id.get());
    assert_eq!(
        listed[0].path, None,
        "a host that holds no tree claimed a path to one"
    );
}

// t[verify files.peering.replication]
#[tokio::test]
async fn hosting_the_same_root_twice_is_not_an_error() {
    let (_data, backend, id) = host().await;
    // A peer re-runs reconciliation without asking first, so this has to
    // be idempotent rather than merely tolerable.
    backend
        .host_structure(id, "Album".into(), RootFlavor::Media)
        .await
        .expect("hosting an already-hosted root");
    assert_eq!(RootsService::list(&backend).await.unwrap().len(), 1);
}

// t[verify files.peering.replication]
#[tokio::test]
async fn structure_questions_answer_and_content_questions_refuse() {
    let (_data, backend, id) = host().await;

    // Structure: an empty catalogue is the honest answer here — this
    // host knows the root exists and has not been told what is in it.
    // Crucially it is an *answer*, not a missing root.
    let listed = TreeService::browse(&backend, id, p(""))
        .await
        .expect("a hosted root must browse, even knowing nothing yet");
    assert!(listed.is_empty());

    // Content: a write has nowhere to go, and finding that out halfway
    // through a batch would leave a partial one.
    let refused = <FilesBackend as WriteService>::create_dirs(&backend, id, vec![p("Stems")]).await;
    assert!(refused.is_err(), "a host with no tree accepted a write");
}

/// The hazard the `Option` exists to prevent, stated as a test.
// t[verify files.peering.replication]
#[tokio::test]
async fn an_unplaced_root_never_touches_the_working_directory() {
    let (_data, backend, id) = host().await;

    // If any call site treated the absent path as `""`, this would be
    // created relative to the process's cwd — the repository, when the
    // test suite runs.
    let canary = std::env::current_dir().unwrap().join("Stems");
    assert!(!canary.exists(), "fixture: the canary already exists");

    let _ = <FilesBackend as WriteService>::create_dirs(&backend, id, vec![p("Stems")]).await;
    let _ = TreeService::browse(&backend, id, p("Stems")).await;
    let _ = FilesService::get_root(&backend, id.get()).await;

    assert!(
        !canary.exists(),
        "an unplaced root resolved against the working directory: {}",
        canary.display()
    );
}
