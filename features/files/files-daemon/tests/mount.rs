//! The cloud folder, at the seam the app and CLI actually drive it
//! from: mount / unmount / mounts over `DaemonControlService`, and the
//! behaviour that makes a mount worth having — a file whose bytes this
//! machine gave back still lists at its real size, and reading it
//! brings the bytes back without anybody asking.
//!
//! # Why this test skips rather than fails
//!
//! Mounting needs `/dev/fuse` and the `fusermount3` helper. A developer
//! machine has both; a container without `--device /dev/fuse` has
//! neither, and a filesystem test that cannot mount is not a failing
//! filesystem — it is an absent kernel feature. The skip says so on
//! stdout so a run that quietly covered nothing is legible as such.

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, FilesService as _, RootFlavor};
use files_daemon::service::{DaemonControlServiceClient, layer as control_layer};
use files_daemon::{DaemonControl, SyncDaemon};

/// Can this machine mount at all?
fn fuse_is_available() -> bool {
    if !std::path::Path::new("/dev/fuse").exists() {
        println!("skipping: no /dev/fuse on this machine");
        return false;
    }
    let helper = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| std::path::Path::new(d).join("fusermount3").exists());
    if !helper {
        println!("skipping: fusermount3 is not on PATH");
    }
    helper
}

struct Rig {
    _dir: tempfile::TempDir,
    root_id: uuid::Uuid,
    tree: std::path::PathBuf,
    mountpoint: std::path::PathBuf,
    control: DaemonControlServiceClient,
    _server: LocalServer,
}

/// One machine holding one checkpointed root — no peer, because
/// mounting is about what a machine does with content it already has a
/// record of, not about where that record came from.
async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).unwrap();

    let tree = dir.path().join("session");
    std::fs::create_dir(&tree).unwrap();
    // Big enough that "did the size survive eviction" is a real
    // question — a stub is a couple of hundred bytes.
    std::fs::write(&tree.join("mix.wav"), vec![7u8; 3 * 1024 * 1024]).unwrap();
    std::fs::write(&tree.join("notes.txt"), b"session notes").unwrap();
    let root = backend
        .create_root(
            tree.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    backend.checkpoint_now(root.id, None).await.unwrap();

    let daemon = SyncDaemon::open(backend, dir.path().join("daemon")).unwrap();
    let scope = Scope::new();
    let server = LocalServer::serve(
        LayerRouter::new().merge(control_layer(DaemonControl::new(daemon))),
        scope,
    );
    let control: DaemonControlServiceClient = server.establish().await.unwrap();

    Rig {
        root_id: root.id,
        tree,
        mountpoint: dir.path().join("mnt"),
        control,
        _server: server,
        _dir: dir,
    }
}

/// The whole point, in one pass: evict a file, and through the mount it
/// is still its full size and still readable — the read is what fetches
/// it. This is what a DAW opening a take it does not have must see.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_whose_bytes_were_given_back_reads_whole_through_the_mount() {
    if !fuse_is_available() {
        return;
    }
    let rig = rig().await;
    let full = 3 * 1024 * 1024;

    // Give the bytes back. On disk it is now a pointer stub — small,
    // and (deliberately) nothing like the file it stands for.
    rig.control
        .dehydrate(rig.root_id, "mix.wav".into())
        .await
        .unwrap();
    let on_disk = std::fs::metadata(rig.tree.join("mix.wav")).unwrap().len();
    assert!(
        on_disk < 4096,
        "eviction should have left a stub, found {on_disk} bytes"
    );

    rig.control
        .mount(rig.root_id, rig.mountpoint.to_string_lossy().into_owned())
        .await
        .unwrap();

    // Through the mount, the listing tells the truth about the file
    // rather than about the placeholder.
    let seen = std::fs::metadata(rig.mountpoint.join("mix.wav"))
        .unwrap()
        .len();
    assert_eq!(
        seen, full,
        "a dehydrated file must list at its real size, not its stub's"
    );

    // And reading it gets the file. Nothing asked for hydration: the
    // open did.
    let bytes = std::fs::read(rig.mountpoint.join("mix.wav")).unwrap();
    assert_eq!(bytes.len(), full as usize);
    assert!(bytes.iter().all(|b| *b == 7), "the content came back wrong");

    // The bytes are resident now — the fetch materialized into the tree
    // rather than being conjured for one reader.
    let after = std::fs::metadata(rig.tree.join("mix.wav")).unwrap().len();
    assert_eq!(after, full);

    rig.control.unmount(rig.root_id).await.unwrap();
}

/// A mount is a window, not the thing: taking it down leaves the tree
/// exactly where it was, and the agent stops claiming it is mounted.
#[tokio::test(flavor = "multi_thread")]
async fn unmounting_leaves_the_files_where_they_are() {
    if !fuse_is_available() {
        return;
    }
    let rig = rig().await;

    rig.control
        .mount(rig.root_id, rig.mountpoint.to_string_lossy().into_owned())
        .await
        .unwrap();
    let mounted = rig.control.mounts().await.unwrap();
    assert_eq!(mounted.len(), 1);
    assert_eq!(mounted[0].0, rig.root_id);

    rig.control.unmount(rig.root_id).await.unwrap();
    assert!(rig.control.mounts().await.unwrap().is_empty());

    // The tree is untouched — this is the promise the CLI prints.
    assert_eq!(
        std::fs::read(rig.tree.join("notes.txt")).unwrap(),
        b"session notes"
    );
}

/// Mounting a root twice is a leaked session, not a second window: the
/// agent keeps one mount per root, so the second insert would drop the
/// first handle and unmount what somebody is looking at. It must refuse.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_cannot_be_mounted_twice() {
    if !fuse_is_available() {
        return;
    }
    let rig = rig().await;
    let at = rig.mountpoint.to_string_lossy().into_owned();

    rig.control.mount(rig.root_id, at.clone()).await.unwrap();
    let again = rig.control.mount(rig.root_id, at).await;
    assert!(again.is_err(), "the second mount should have been refused");

    // And the first one is still standing.
    assert_eq!(rig.control.mounts().await.unwrap().len(), 1);
    rig.control.unmount(rig.root_id).await.unwrap();
}

/// Unmounting something that is not mounted is an error a person can
/// act on, not a silent success that leaves them believing they undid
/// something.
#[tokio::test(flavor = "multi_thread")]
async fn unmounting_what_is_not_mounted_says_so() {
    let rig = rig().await;
    assert!(rig.control.unmount(rig.root_id).await.is_err());
}
