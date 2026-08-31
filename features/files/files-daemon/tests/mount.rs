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

/// The claim the whole layout rests on: **where a root appears has
/// nothing to do with where its bytes are.**
///
/// A studio's disk is laid out by fifteen years of accidents — an old
/// export here, a rescued drive there, one client's work on a NAS
/// because that is where the space was. The tree a person should see is
/// not that, and reshaping terabytes to make the two agree would be an
/// expensive answer to a question about presentation.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_appears_where_it_is_placed_not_where_it_lives() {
    if !fuse_is_available() {
        return;
    }
    let rig = rig().await;

    // The tree is at <tmp>/session. It will appear somewhere with no
    // resemblance to that.
    rig.control
        .set_place(rig.root_id, "codywright/Projects/Some Record".into())
        .await
        .unwrap();

    let under = rig.mountpoint.join("Task");
    let outcomes = rig
        .control
        .mount_all(under.to_string_lossy().into_owned(), false)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].0, "codywright/Projects/Some Record");
    assert!(outcomes[0].1.is_none(), "{:?}", outcomes[0].1);

    // The composed tree exists, with the project's own files under it.
    let shown = under.join("codywright/Projects/Some Record");
    assert_eq!(
        std::fs::read(shown.join("notes.txt")).unwrap(),
        b"session notes"
    );
    // And the tree it came from is exactly where it always was.
    assert!(rig.tree.join("notes.txt").exists());
    assert!(!rig.tree.to_string_lossy().contains("codywright"));

    // One mount covers every root, so it is not registered under any
    // root's id — taking it down means taking down what is mounted.
    let mounted = rig.control.mounts().await.unwrap();
    assert_eq!(mounted.len(), 1, "the whole tree is one mount");
    assert_ne!(
        mounted[0].0, rig.root_id,
        "the composed tree is not any one root"
    );
    rig.control.unmount(mounted[0].0).await.unwrap();
}

/// A place that climbs out of the tree would mount a root anywhere on
/// the disk, from a string that looks like a folder name.
#[tokio::test(flavor = "multi_thread")]
async fn a_place_cannot_escape_the_tree() {
    let rig = rig().await;
    for bad in ["../../etc", "org/../../..", "", "/"] {
        assert!(
            rig.control
                .set_place(rig.root_id, bad.into())
                .await
                .is_err(),
            "`{bad}` should not be a place"
        );
    }
    assert!(
        rig.control
            .set_place(rig.root_id, "org/Projects/Fine".into())
            .await
            .is_ok()
    );
}

/// Unmounting something that is not mounted is an error a person can
/// act on, not a silent success that leaves them believing they undid
/// something.
#[tokio::test(flavor = "multi_thread")]
async fn unmounting_what_is_not_mounted_says_so() {
    let rig = rig().await;
    assert!(rig.control.unmount(rig.root_id).await.is_err());
}

/// Tags reach the filesystem, and they are **derived** rather than
/// stored: the org comes from the root's place, a note's own tags come
/// from its frontmatter. Neither is written to disk as an xattr, so
/// nothing can drift from the note that is the authority.
#[tokio::test(flavor = "multi_thread")]
async fn tags_reach_the_filesystem_without_being_stored_there() {
    if !fuse_is_available() {
        return;
    }
    let rig = rig().await;
    std::fs::write(
        rig.tree.join("note.md"),
        "---\ntitle: A note\ntags: [mixing, urgent]\n---\n\nbody\n",
    )
    .unwrap();

    rig.control
        .set_place(rig.root_id, "acme/Projects/Session".into())
        .await
        .unwrap();
    rig.control
        .mount(rig.root_id, rig.mountpoint.to_string_lossy().into_owned())
        .await
        .unwrap();

    // The org, on anything in the root.
    let on_audio = xattr::get(rig.mountpoint.join("mix.wav"), "user.xdg.tags")
        .unwrap()
        .expect("a file in a placed root carries its org");
    assert_eq!(String::from_utf8(on_audio).unwrap(), "acme");

    // The org *and* the note's own tags, on the note.
    let on_note = xattr::get(rig.mountpoint.join("note.md"), "user.xdg.tags")
        .unwrap()
        .expect("a note carries its frontmatter tags");
    assert_eq!(
        String::from_utf8(on_note).unwrap(),
        "acme,mixing,urgent",
        "the org first, then what the note declares"
    );

    // Derived, not stored: nothing wrote an xattr onto the real file.
    assert!(
        xattr::get(rig.tree.join("note.md"), "user.xdg.tags")
            .unwrap()
            .is_none(),
        "the tags must not have been written to disk — the note is the authority"
    );

    rig.control.unmount(rig.root_id).await.unwrap();
}

/// Making a folder where projects live makes a project.
///
/// The vocabulary a file manager has is folders, so that is the
/// vocabulary this answers in: a new folder beside existing projects
/// becomes a root, adopted and placed, with its bytes beside its
/// siblings' — and a new folder *inside* a project stays an ordinary
/// folder, because that is what somebody meant by it.
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_made_beside_projects_becomes_one() {
    if !fuse_is_available() {
        return;
    }
    let rig = rig().await;
    rig.control
        .set_place(rig.root_id, "acme/Projects/First".into())
        .await
        .unwrap();

    let under = rig.mountpoint.join("Task");
    rig.control
        .mount_all(under.to_string_lossy().into_owned(), false)
        .await
        .unwrap();

    // Beside an existing project.
    let made = under.join("acme/Projects/Second");
    std::fs::create_dir(&made).unwrap();

    let placed = rig.control.placed_roots().await.unwrap();
    let second = placed
        .iter()
        .find(|r| r.place == "acme/Projects/Second")
        .expect("the new folder should have become a root");
    assert_eq!(second.name, "Second");

    // Whoever made the folder made the project. The kernel says who
    // asked, and this is the only moment that answer is free — a week
    // later nothing on disk knows.
    let maker = second
        .made_by
        .as_ref()
        .expect("a project made here should know who made it");
    assert_eq!(
        maker.uid,
        // SAFETY: getuid is always safe; it reads this process's own id.
        unsafe { libc::getuid() },
        "the maker is whoever asked, not whoever the agent runs as"
    );
    assert!(!maker.user.is_empty());
    assert!(maker.device.is_some(), "and which machine it happened on");

    // A root that simply exists claims no maker rather than inventing
    // one — the honest answer for anything that arrived from a peer.
    let first = placed.iter().find(|r| r.place == "acme/Projects/First").unwrap();
    assert!(first.made_by.is_none());
    // Its bytes sit beside its sibling's, wherever that turned out to
    // be — not anywhere derived from the place string.
    let sibling = std::path::Path::new(&rig.tree).parent().unwrap();
    assert_eq!(
        std::path::Path::new(&second.path).parent().unwrap(),
        sibling,
        "a new project belongs beside the ones already there"
    );
    assert!(std::path::Path::new(&second.path).is_dir());

    // And it is browsable at once, through the same mount.
    assert!(made.is_dir());

    // It declares itself the moment it exists — no app, no session, no
    // window open anywhere. Everything here is derivable at the instant
    // the folder appears.
    let page = std::fs::read_to_string(made.join("project.md"))
        .expect("a new project declares itself when it is made");
    assert!(page.starts_with("---\n"), "frontmatter first: {page}");
    assert!(page.contains("title: Second"), "{page}");
    assert!(page.contains("organization: acme"), "{page}");
    assert!(page.contains("createdBy: "), "{page}");
    // `lead` is the one thing a service cannot know, so it is left for
    // the app rather than guessed.
    assert!(
        !page.contains("lead:"),
        "a service has no session and must not invent an owner: {page}"
    );

    // Inside a project, a folder is just a folder.
    let ordinary = under.join("acme/Projects/First/Stems");
    std::fs::create_dir(&ordinary).unwrap();
    assert!(rig.tree.join("Stems").is_dir(), "it belongs to the project");
    assert!(
        !rig.control
            .placed_roots()
            .await
            .unwrap()
            .iter()
            .any(|r| r.place.ends_with("Stems")),
        "an ordinary folder must not become a root"
    );

    let mounted = rig.control.mounts().await.unwrap();
    rig.control.unmount(mounted[0].0).await.unwrap();
}
