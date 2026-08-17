//! The catalogue outliving both the process and the filesystem.
//!
//! `files.catalogue.offline` promises a browse works with the holding
//! location absent, and `files.peering.replication` needs a host to hold
//! an org's structure while holding none of its content. Those are the
//! same capability: knowing what is there without being able to see it.
//!
//! Both were unreachable while the catalogue was process-global and
//! built by walking the live tree on first use — a cold process asked
//! the disk before it could answer, so "the disk is gone" and "the tree
//! is empty" produced the same reply. These tests are written to fail if
//! that regresses, which means each one destroys something before it
//! asks.

use files::{FilesBackend, FilesService, RootFlavor};
use files_proto::id::RootId;
use files_proto::path::RootPath;
use files_proto::service::tree::TreeService;
use files_proto::service::write::WriteService;

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

/// A root with a small tree, catalogued.
///
/// The data dir and the tree are separate directories on purpose: the
/// tests below delete the tree and keep the data dir, which is exactly
/// the shape of a host that holds structure and no content.
async fn rig() -> (tempfile::TempDir, std::path::PathBuf, FilesBackend, RootId) {
    let data = tempfile::tempdir().expect("data dir");
    let backend = FilesBackend::new(data.path(), data.path().join("vault")).expect("backend");

    let tree = data.path().join("album");
    std::fs::create_dir_all(tree.join("Audio Files")).unwrap();
    std::fs::write(tree.join("Song.rpp"), b"REAPER project").unwrap();
    std::fs::write(tree.join("Audio Files").join("vox.wav"), vec![7u8; 4096]).unwrap();

    let root = FilesService::create_root(
        &backend,
        tree.to_string_lossy().into_owned(),
        "Album".into(),
        RootFlavor::Media,
    )
    .await
    .expect("create root");
    let root = RootId::new(root.id);

    // Ask a catalogue question, which is what builds and persists it.
    TreeService::catalogue(&backend, root, None)
        .await
        .expect("catalogue");

    (data, tree, backend, root)
}

// t[verify files.catalogue.offline]
#[tokio::test]
async fn a_browse_answers_with_the_tree_gone() {
    let (_data, tree, backend, root) = rig().await;
    assert!(
        TreeService::browse(&backend, root, p("")).await.is_ok(),
        "sanity: it works first"
    );

    // The holding location goes away. Not a rename — gone.
    std::fs::remove_dir_all(&tree).unwrap();

    let listed = TreeService::browse(&backend, root, p(""))
        .await
        .expect("a folder whose location is down must list, not fail");
    let mut names: Vec<&str> = listed.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["Audio Files", "Song.rpp"],
        "the tree read as empty rather than as elsewhere"
    );
}

// t[verify files.catalogue.offline]
#[tokio::test]
async fn an_absent_location_is_a_fact_about_content_not_about_the_tree() {
    let (_data, tree, backend, root) = rig().await;
    std::fs::remove_dir_all(&tree).unwrap();

    let listed = TreeService::browse(&backend, root, p("Audio Files"))
        .await
        .expect("browse");
    let vox = listed
        .iter()
        .find(|e| e.name == "vox.wav")
        .expect("the file is still known to exist");

    // Marked unavailable, not resident: the entry is real and its bytes
    // are not here, and claiming otherwise sends a caller to open a file
    // that is not on this machine.
    assert!(vox.stub, "an entry we cannot read was reported as resident");
    // But its size is the truth, because "how big is this project" must
    // not answer zero on a host that holds no bytes.
    assert_eq!(vox.size, Some(4096));
}

// t[verify files.catalogue.offline]
#[tokio::test]
async fn the_catalogue_outlives_the_process() {
    let (data, tree, _backend, root) = rig().await;
    // Both at once: a second backend on the same data dir is the restart,
    // and the missing tree means it cannot quietly rebuild by walking.
    // Passing this while reading a cached in-memory copy is the failure
    // mode, so the tree has to be gone before the new backend exists.
    std::fs::remove_dir_all(&tree).unwrap();

    let restarted =
        FilesBackend::new(data.path(), data.path().join("vault")).expect("restarted backend");
    let listed = TreeService::browse(&restarted, root, p(""))
        .await
        .expect("a restart lost the catalogue it had already built");
    assert_eq!(listed.len(), 2);
}

// t[verify files.catalogue.complete]
#[tokio::test]
async fn a_write_reaches_the_durable_catalogue_not_only_the_served_one() {
    let (data, tree, backend, root) = rig().await;
    <FilesBackend as WriteService>::create_dirs(&backend, root, vec![p("Mixes")])
        .await
        .expect("create a folder");

    // A durable copy that lags the served one is worse than none: a
    // restart would answer confidently with a tree one write out of
    // date. So the check is against a *different* backend, after the
    // tree is gone — the only way to be sure the answer came off disk.
    std::fs::remove_dir_all(&tree).unwrap();
    let restarted =
        FilesBackend::new(data.path(), data.path().join("vault")).expect("restarted backend");
    let names: Vec<String> = TreeService::browse(&restarted, root, p(""))
        .await
        .expect("browse")
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(
        names.iter().any(|n| n == "Mixes"),
        "the write never reached disk: {names:?}"
    );
}

// t[verify files.catalogue.offline]
#[tokio::test]
async fn the_catalogue_is_on_disk_and_not_merely_in_this_process() {
    let (data, _tree, _backend, root) = rig().await;

    // The two tests above construct a second backend to stand for a
    // restart, and that is not sufficient on its own: the in-memory
    // catalogue map is process-global and keyed by `RootId`, and
    // `Scoped` caches by data directory, so a second backend in this
    // process can answer from memory having never read a byte. This
    // reads the file itself, which is the only claim that survives.
    let file = data.path().join("catalogue.json");
    let raw = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    assert!(
        raw.contains(&root.to_string()),
        "the durable catalogue does not mention the root it catalogued"
    );
    assert!(
        raw.contains("vox.wav"),
        "entries did not reach disk: {raw:.400}"
    );
}
