//! Nested roots are **submodules**: a project inside a project, each
//! with its own history, and the parent keeping files of its own.
//!
//! This is the shape the work is actually organised in — an album over
//! its songs, a venue over its shows, a client over two tracking
//! sessions. It used to be refused outright, for a real reason: an outer
//! root's checkpoint would walk into the inner root and ingest its whole
//! version store as ordinary content, duplicating the child's history
//! inside the parent's on every capture.
//!
//! So the prune and the relaxed containment check are one feature. These
//! tests pin both halves, and the third test is the one that matters: a
//! parent may hold its own files *while* containing a child root, which
//! is the whole point of allowing the nesting at all.

use std::path::{Path, PathBuf};

use files::{FilesBackend, MARKER_FILE};
use files_proto::{FilesService as _, RootFlavor};

struct Fixture {
    _tmp: tempfile::TempDir,
    files_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let files_dir = tmp.path().join("files");
    std::fs::create_dir_all(&files_dir).expect("mkdir");
    Fixture {
        _tmp: tmp,
        files_dir,
    }
}

fn backend(f: &Fixture) -> FilesBackend {
    let vault = f.files_dir.parent().expect("parent").join("vault");
    std::fs::create_dir_all(&vault).expect("vault");
    FilesBackend::new(&f.files_dir, &vault).expect("backend")
}

fn write(path: &Path, body: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

#[tokio::test]
async fn a_root_may_be_created_inside_another_root() {
    let f = fixture();
    let b = backend(&f);
    let album = f.files_dir.join("Golden Hour");
    let song = album.join("Track 01");
    std::fs::create_dir_all(&song).expect("mkdir");

    b.create_root(
        album.to_string_lossy().into_owned(),
        "Golden Hour".into(),
        RootFlavor::Media,
    )
    .await
    .expect("the album registers");

    b.create_root(
        song.to_string_lossy().into_owned(),
        "Track 01".into(),
        RootFlavor::Media,
    )
    .await
    .expect("a song inside the album registers — this is a submodule");

    assert_eq!(b.list_roots().await.expect("list").len(), 2);
}

#[tokio::test]
async fn the_exact_same_path_is_still_refused() {
    // Relaxing containment must not relax identity: two roots over the
    // same directory would be two histories of one tree.
    let f = fixture();
    let b = backend(&f);
    let dir = f.files_dir.join("Golden Hour");
    std::fs::create_dir_all(&dir).expect("mkdir");

    b.create_root(
        dir.to_string_lossy().into_owned(),
        "Golden Hour".into(),
        RootFlavor::Media,
    )
    .await
    .expect("first");

    b.create_root(
        dir.to_string_lossy().into_owned(),
        "Again".into(),
        RootFlavor::Media,
    )
    .await
    .expect_err("the same directory twice is still a conflict");
}

#[tokio::test]
async fn a_parent_keeps_its_own_files_and_does_not_swallow_the_child() {
    // The point of the whole feature: the container holds its own
    // material (a contract, a roster, notes) AND contains child
    // projects, without the child's content or history bleeding into
    // the parent's checkpoint.
    let f = fixture();
    let b = backend(&f);
    let album = f.files_dir.join("Golden Hour");
    let song = album.join("Track 01");
    std::fs::create_dir_all(&song).expect("mkdir");

    write(&album.join("liner-notes.md"), "the album's own file\n");
    write(&song.join("mix.wav"), "the song's own file\n");

    let album_root = b
        .create_root(
            album.to_string_lossy().into_owned(),
            "Golden Hour".into(),
            RootFlavor::Media,
        )
        .await
        .expect("album");
    b.create_root(
        song.to_string_lossy().into_owned(),
        "Track 01".into(),
        RootFlavor::Media,
    )
    .await
    .expect("song");

    b.checkpoint_now(album_root.id, Some("album".into()))
        .await
        .expect("the album checkpoints with a child root inside it");

    // The parent's own file IS versioned by the parent…
    let own = b
        .chain(album_root.id, "liner-notes.md".into())
        .await
        .expect("chain for the parent's own file");
    assert!(
        !own.is_empty(),
        "the parent's own files are still its content"
    );

    // …and nothing inside the child is. This is the property the prune
    // exists for: the child's content and — far worse — its version
    // store must never enter the parent's history.
    //
    // Asserted against the CHECKPOINTED tree, not `browse`: browse lists
    // the live filesystem, so the parent can of course still see the
    // child's directory on disk. What must not happen is the parent
    // *tracking* it.
    for leaked in ["Track 01/mix.wav", &format!("Track 01/{MARKER_FILE}")] {
        let chain = b
            .chain(album_root.id, leaked.to_owned())
            .await
            .unwrap_or_default();
        assert!(
            chain.is_empty(),
            "the parent tracked `{leaked}` from inside a child root"
        );
    }
}

#[tokio::test]
async fn the_child_still_versions_its_own_content() {
    // The other half of the same property: pruning the child out of the
    // parent must not make the child's own checkpoint miss anything.
    let f = fixture();
    let b = backend(&f);
    let album = f.files_dir.join("Golden Hour");
    let song = album.join("Track 01");
    std::fs::create_dir_all(&song).expect("mkdir");
    write(&song.join("mix.wav"), "the song's own file\n");

    b.create_root(
        album.to_string_lossy().into_owned(),
        "Golden Hour".into(),
        RootFlavor::Media,
    )
    .await
    .expect("album");
    let song_root = b
        .create_root(
            song.to_string_lossy().into_owned(),
            "Track 01".into(),
            RootFlavor::Media,
        )
        .await
        .expect("song");

    b.checkpoint_now(song_root.id, Some("song".into()))
        .await
        .expect("child checkpoints");

    let listing = b.browse(song_root.id, String::new()).await.expect("browse");
    let names: Vec<&str> = listing.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"mix.wav"),
        "the child owns its own content: {names:?}"
    );
}
