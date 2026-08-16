//! The Files RPC surface v1 (issue #259) end to end over an in-process
//! `architect::LocalServer` — the spec's Testing Decisions primary
//! seam ("the established idiom ... the session facade's memory-link
//! bootstrap tests are the prior art"), mirroring `task`'s own
//! `tests/events_stream.rs`.
//!
//! Covers every acceptance criterion, plus the PR #280 review's
//! findings: org confinement (`create_root`/`drive_browse` can't reach
//! outside the org's own files area), root-browse escape (absolute
//! subpath, `..`, symlink), nested-root rejection, `changed_paths`
//! accuracy, the concurrent-checkpoint race, and root identity
//! surviving a genuine `FilesBackend` restart.
//!
//! The second `FilesBackend::new` argument is the org vault holding
//! the curated version entities (issue #261) — irrelevant to
//! everything here, so these tests point it at a directory beside the
//! roots rather than staging a whole vault. The curation surface has
//! its own file, `versions_rpc.rs`.

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::FilesServiceStreamSource as _;
use files::{
    FileRootInfo, FilesBackend, FilesEvent, FilesServiceClient, FilesServiceStreamClient,
    RootFlavor, files_service_layer, files_service_stream_layer,
};

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files_service_layer(backend.clone()))
        .merge(files_service_stream_layer(backend))
}

async fn next_event(rx: &mut vox::Rx<FilesEvent>) -> FilesEvent {
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for a FilesEvent")
        .expect("event channel errored")
        .expect("event stream closed early");
    let mut copied = None;
    let _ = frame.map(|ev| copied = Some(ev));
    copied.expect("SelfRef::map ran")
}

#[tokio::test(flavor = "multi_thread")]
async fn create_browse_chain_checkpoint_over_rpc() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    // Roots are confined to the org's own files area (PR #280 review) —
    // `data_dir` doubles as both `FilesBackend`'s data root and its
    // confinement boundary, so a producer's "existing folder" must
    // already be staged somewhere under it (e.g. an inbox the sync
    // daemon populated) rather than anywhere on the server.
    let root_dir = data_dir.path().join("mix-session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(root_dir.join("stems")).unwrap();
    std::fs::write(root_dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());

    let client: FilesServiceClient = local
        .establish()
        .await
        .expect("establish FilesServiceClient");
    let stream: FilesServiceStreamClient = local
        .establish()
        .await
        .expect("establish FilesServiceStreamClient");

    // Subscribe before mutating (the call stays in flight for the life
    // of the subscription — see `task`'s `events_stream.rs` for why).
    let (tx, mut rx) = vox::channel::<FilesEvent>();
    let subscription = tokio::spawn(async move {
        stream.events(tx).await.expect("subscribe to files events");
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while backend.events_hub().subscriber_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("subscriber sink never reached the backend hub");

    // create_root: marker file + entity.
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Mix Session".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");
    assert_eq!(root.name, "Mix Session");
    assert!(
        root_dir.join(".fts-root.json").exists(),
        "marker file written into the root's own tree"
    );
    match next_event(&mut rx).await {
        FilesEvent::RootCreated(r) => assert_eq!(r.id, root.id),
        other => panic!("expected RootCreated, got {other:?}"),
    }

    // A second create_root on the same folder is rejected — root
    // identity is unique per tree.
    let dup = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Dup".to_string(),
            RootFlavor::Media,
        )
        .await;
    assert!(
        dup.is_err(),
        "creating a root over an existing root must fail"
    );

    // list_roots / get_root.
    let listed = client.list_roots().await.expect("list_roots rpc");
    assert_eq!(listed.len(), 1);
    let got = client.get_root(root.id).await.expect("get_root rpc");
    assert_eq!(got.path, root.path);

    // browse (root-scoped) hides the marker file / store dir; a nested
    // subpath is a distinct call.
    let top = client
        .browse(root.id, String::new())
        .await
        .expect("browse rpc");
    let names: Vec<_> = top.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"mix.wav"));
    assert!(names.contains(&"stems"));
    assert!(
        !names.contains(&".fts-root.json") && !names.contains(&".fts-files"),
        "root browsing hides internals: {names:?}"
    );
    let stems = client
        .browse(root.id, "stems".to_string())
        .await
        .expect("browse rpc");
    assert_eq!(stems.len(), 1);
    assert_eq!(stems[0].name, "kick.wav");

    // drive_browse (rootless, still org-confined) is a genuinely
    // different view — it shows the raw tree, internals included.
    let drive = client
        .drive_browse(root_dir.to_str().unwrap().to_string())
        .await
        .expect("drive_browse rpc");
    let drive_names: Vec<_> = drive.iter().map(|e| e.name.as_str()).collect();
    assert!(
        drive_names.contains(&".fts-root.json"),
        "drive_browse shows the raw tree: {drive_names:?}"
    );

    // checkpoint_now — the live tree checkpoints; chain sees it.
    let cp1 = client
        .checkpoint_now(root.id, Some("first save".to_string()))
        .await
        .expect("checkpoint_now rpc");
    assert!(cp1.changed_paths.contains(&"mix.wav".to_string()));
    assert!(cp1.changed_paths.contains(&"stems/kick.wav".to_string()));
    match next_event(&mut rx).await {
        FilesEvent::Checkpointed(info) => assert_eq!(info.commit_id, cp1.commit_id),
        other => panic!("expected Checkpointed, got {other:?}"),
    }

    let chain = client
        .chain(root.id, "mix.wav".to_string())
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 1, "one saved state so far: {chain:?}");
    assert_eq!(chain[0].commit_id, cp1.commit_id);

    // Edit the file and checkpoint again — a second chain entry.
    std::fs::write(root_dir.join("mix.wav"), b"take two, final").unwrap();
    let cp2 = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint_now rpc");
    assert_eq!(cp2.description, "checkpoint now", "default description");
    assert_ne!(cp2.commit_id, cp1.commit_id);
    assert_eq!(
        cp2.changed_paths,
        vec!["mix.wav".to_string()],
        "only the edited file is reported changed"
    );

    let chain = client
        .chain(root.id, "mix.wav".to_string())
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 2, "two saved states now: {chain:?}");
    assert_eq!(chain[0].commit_id, cp2.commit_id, "newest first");
    assert_eq!(chain[1].commit_id, cp1.commit_id);

    // A checkpoint with no live-tree changes still succeeds (a new
    // commit — the certifying scan itself is the event), but writes
    // nothing: `changed_paths` is empty (its own documented contract),
    // and no new chain entry appears.
    let cp3 = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint_now rpc");
    assert_ne!(cp3.commit_id, cp2.commit_id, "still a new commit");
    assert!(
        cp3.changed_paths.is_empty(),
        "a no-op checkpoint changes nothing: {:?}",
        cp3.changed_paths
    );
    let chain_after_noop = client
        .chain(root.id, "mix.wav".to_string())
        .await
        .expect("chain rpc");
    assert_eq!(
        chain_after_noop.len(),
        2,
        "an unchanged file gains no new chain entry: {chain_after_noop:?}"
    );

    subscription.abort();
    scope.close().await;
}

/// PR #280 review findings 1+2: `create_root` and `drive_browse` must
/// not reach outside the org's own files area, and root-scoped
/// `browse` must not escape the root via an absolute subpath, `..`, or
/// a symlink.
#[tokio::test(flavor = "multi_thread")]
async fn filesystem_access_is_confined() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    std::fs::write(outside.path().join("secret.txt"), b"org B's data").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    // create_root outside the org's files area is rejected outright.
    let outside_create = client
        .create_root(
            outside.path().to_str().unwrap().to_string(),
            "Escape".to_string(),
            RootFlavor::Media,
        )
        .await;
    assert!(
        outside_create.is_err(),
        "create_root must reject a path outside the org's files area"
    );
    assert!(
        !outside.path().join(".fts-root.json").exists(),
        "a rejected create_root must not write a marker file outside confinement"
    );

    // drive_browse outside the org's files area is rejected outright —
    // no server-filesystem enumeration at plain member tier.
    let outside_browse = client
        .drive_browse(outside.path().to_str().unwrap().to_string())
        .await;
    assert!(
        outside_browse.is_err(),
        "drive_browse must reject a path outside the org's files area"
    );

    // A real root, to probe browse's escape guard against.
    let root_dir = data_dir.path().join("root-a");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("inside.txt"), b"ok").unwrap();
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Root A".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");

    // Absolute subpath: `root_path.join("/etc")` replaces the base
    // entirely under plain `PathBuf::join` semantics — must still be
    // rejected, not silently resolve to `/etc`.
    let abs_escape = client.browse(root.id, "/etc".to_string()).await;
    assert!(
        abs_escape.is_err(),
        "an absolute subpath must not escape the root"
    );

    // `..`-relative escape.
    let dotdot_escape = client.browse(root.id, "../".to_string()).await;
    assert!(
        dotdot_escape.is_err(),
        "a `..` subpath must not escape the root"
    );

    // Symlink escape: a symlink *inside* the root pointing at
    // `outside` — canonicalizing the resolved target must catch this
    // even though the textual path never leaves the root.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), root_dir.join("escape-link")).unwrap();
        let symlink_escape = client.browse(root.id, "escape-link".to_string()).await;
        assert!(
            symlink_escape.is_err(),
            "a symlink inside the root pointing outside it must not be followed out"
        );
    }

    scope.close().await;
}

/// Nested roots are **submodules**, in both directions.
///
/// This test previously asserted the opposite — PR #280 review finding 5
/// refused a root inside (or around) an existing root, because an outer
/// root's checkpoint would walk into the inner one and ingest its entire
/// version store as ordinary content. That reasoning was correct, and
/// the refusal was the right answer *until the walk learned to prune*.
///
/// It now does: `scan::walk_live_tree` skips any directory carrying
/// another root's marker, so the parent walks around its children.
/// Containment is therefore allowed and only exact-path duplication is
/// refused. The properties that finding protected are pinned in
/// `tests/nested_roots.rs` — a parent keeps its own files, and tracks
/// nothing from inside a child.
///
/// The nesting matters because it is the shape the work is in: an album
/// over its songs, a venue over its shows. Each child keeps its own
/// history and its own share link; the parent keeps material of its own.
#[tokio::test(flavor = "multi_thread")]
async fn nested_roots_are_submodules() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    // Outer root first; a descendant root is now permitted.
    let outer = data_dir.path().join("outer");
    std::fs::create_dir(&outer).unwrap();
    client
        .create_root(
            outer.to_str().unwrap().to_string(),
            "Outer".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create outer root");

    let inner = outer.join("inner");
    std::fs::create_dir(&inner).unwrap();
    let nested = client
        .create_root(
            inner.to_str().unwrap().to_string(),
            "Inner".to_string(),
            RootFlavor::Media,
        )
        .await;
    assert!(
        nested.is_ok(),
        "a root nested inside an existing root is a submodule: {nested:?}"
    );

    // The other direction too: registering the container AFTER its
    // children is the ordinary case when an album's songs were tracked
    // first and the album is added later.
    let parent = data_dir.path().join("parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    client
        .create_root(
            child.to_str().unwrap().to_string(),
            "Child".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create child root");
    let ancestor = client
        .create_root(
            parent.to_str().unwrap().to_string(),
            "Parent".to_string(),
            RootFlavor::Media,
        )
        .await;
    assert!(
        ancestor.is_ok(),
        "a container registered around existing roots is a submodule parent: {ancestor:?}"
    );

    // What is still refused: the same directory twice. Relaxing
    // containment must not relax identity — two roots over one tree
    // would be two histories of the same files.
    let duplicate = client
        .create_root(
            child.to_str().unwrap().to_string(),
            "Child Again".to_string(),
            RootFlavor::Media,
        )
        .await;
    assert!(
        duplicate.is_err(),
        "the same path twice is still a conflict"
    );

    scope.close().await;
}

/// Carving a child root out of a parent that ALREADY tracks those
/// files — the real migration case, not the greenfield one the test
/// above covers. A 5 TB tree gets registered as whole-project roots
/// first; deciding an album's songs deserve their own roots comes
/// later, by which time the album has history.
///
/// What must hold: the files are not lost, the album keeps its own
/// files, and everything captured before the split is still
/// **recoverable** from the commit that captured it. The parent
/// recording a deletion at the moment of the split is correct and
/// expected — the paths genuinely left its tree, exactly as `git rm -r`
/// precedes adding a submodule.
///
/// Note what is NOT asserted: that `chain(parent, "song one/mix.wav")`
/// still answers. It does not, and that is not a carving bug — `chain`
/// walks back from HEAD and stops the moment the path is absent
/// (`chain.rs:112`), so the history of ANY deleted file is unreachable
/// that way. Worth knowing before splitting a tree that already has
/// history: reach it through `browse_at` on the pre-split commit, which
/// is what this pins.
#[tokio::test(flavor = "multi_thread")]
async fn carving_a_child_root_out_of_tracked_files_keeps_the_history() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let album = data_dir.path().join("album");
    let song = album.join("song one");
    std::fs::create_dir_all(&song).unwrap();
    std::fs::write(album.join("album notes.txt"), b"the album's own file").unwrap();
    std::fs::write(song.join("mix.wav"), b"v1 of the song").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let parent = client
        .create_root(
            album.to_str().unwrap().to_string(),
            "Album".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create album root");
    let whole = client
        .checkpoint_now(parent.id, Some("album captured whole".to_string()))
        .await
        .expect("first checkpoint");

    // The song is in the album's history at this point.
    let before = client
        .chain(parent.id, "song one/mix.wav".to_string())
        .await
        .expect("chain rpc");
    assert!(
        !before.is_empty(),
        "the song must be in the album's history before the split"
    );

    // Now carve it out.
    let child = client
        .create_root(
            song.to_str().unwrap().to_string(),
            "Song One".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create song root");
    client
        .checkpoint_now(parent.id, Some("after the split".to_string()))
        .await
        .expect("second checkpoint");

    // The album keeps its OWN files — the whole point of the submodule
    // prune (a parent with subprojects still has files of its own).
    let album_entries = client
        .browse(parent.id, String::new())
        .await
        .expect("browse album");
    assert!(
        album_entries.iter().any(|e| e.name == "album notes.txt"),
        "the album's own files must survive the split: {album_entries:?}"
    );

    // What was already captured is still THERE, reachable at the
    // commit that captured it. This is the property that makes
    // splitting a tree that already has history a safe operation.
    let at_split = client
        .browse_at(parent.id, whole.commit_id.clone(), "song one".to_string())
        .await
        .expect("browse_at the pre-split commit");
    assert!(
        at_split.iter().any(|e| e.name == "mix.wav"),
        "the pre-split capture must still hold the song's file: {at_split:?}"
    );

    // And the child owns the file going forward.
    let song_entries = client
        .browse(child.id, String::new())
        .await
        .expect("browse song");
    assert!(
        song_entries.iter().any(|e| e.name == "mix.wav"),
        "the song root must see its own file: {song_entries:?}"
    );

    scope.close().await;
}

/// Renaming or moving a project folder must not strand its root.
///
/// This is the ordinary case, not an edge one: the whole point of
/// putting an archive under Task is to reorganise it — rename a
/// misspelled client, move a song under the album it belongs to, split
/// a year into folders. A root records the absolute path it was created
/// at, so every one of those breaks the recorded path.
///
/// What saves it is that the marker AND the entire version store live
/// inside the folder and travel with it. Re-adding the folder at its new
/// location must therefore re-point the existing root — same id, same
/// history — rather than refuse it as "already a root", which used to
/// strand it permanently: the old path dead, the real folder
/// un-registrable forever.
#[ignore = "needs a close-the-store seam first: re-opening a moved root \
            hangs on the redb lock its old handle still holds (iroh-blobs \
            runs its own runtime). See plans/media-roots-at-scale.md."]
#[tokio::test(flavor = "multi_thread")]
async fn moving_a_root_folder_re_points_it_and_keeps_its_history() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let original = data_dir.path().join("Yokasta Segura");
    std::fs::create_dir_all(&original).unwrap();
    std::fs::write(original.join("mix.wav"), b"v1").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let root = client
        .create_root(
            original.to_str().unwrap().to_string(),
            "Yokasta Segura".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create root");
    let first = client
        .checkpoint_now(root.id, Some("before the move".to_string()))
        .await
        .expect("checkpoint");

    // Rename it — the spelling correction that actually happened to
    // this material more than once.
    let moved = data_dir.path().join("El Artista Eres Tu - Yokasta Segura");
    std::fs::rename(&original, &moved).unwrap();

    let readded = client
        .create_root(
            moved.to_str().unwrap().to_string(),
            // A different name on the way back in must not matter; the
            // marker is the authority on identity.
            "whatever the caller types".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("re-adding a moved root must succeed, not conflict");

    assert_eq!(
        readded.id, root.id,
        "the same folder is the same root — its id is what versions, \
         reviews and placements reference"
    );
    assert_eq!(readded.path.as_deref(), moved.to_str());

    // Exactly one root, not a second one shadowing the first.
    let roots = client.list_roots().await.expect("list");
    assert_eq!(roots.len(), 1, "re-pointing must not duplicate: {roots:?}");

    // The history came with the folder.
    let chain = client
        .chain(root.id, "mix.wav".to_string())
        .await
        .expect("chain after the move");
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].commit_id, first.commit_id);

    // And it still versions going forward, from the new location.
    std::fs::write(moved.join("mix.wav"), b"v2 after the move").unwrap();
    client
        .checkpoint_now(root.id, Some("after the move".to_string()))
        .await
        .expect("checkpoint at the new path");
    let chain = client
        .chain(root.id, "mix.wav".to_string())
        .await
        .expect("chain");
    assert_eq!(chain.len(), 2, "a move must not end the file's history");

    scope.close().await;
}

/// PR #280 review finding 4: two concurrent `checkpoint_now` calls on
/// the same root must not race — every writer's change must land in
/// the chain, none silently orphaned by a lost `set_head`.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_checkpoints_on_same_root_do_not_race() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("concurrent");
    std::fs::create_dir(&root_dir).unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Concurrent".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");

    const N: usize = 8;
    let mut tasks = Vec::new();
    for i in 0..N {
        std::fs::write(root_dir.join(format!("f{i}.txt")), format!("content {i}")).unwrap();
        let client = client.clone();
        let root_id = root.id;
        tasks.push(tokio::spawn(async move {
            client
                .checkpoint_now(root_id, Some(format!("writer {i}")))
                .await
                .expect("checkpoint_now rpc")
        }));
    }
    let mut commit_ids = std::collections::HashSet::new();
    for t in tasks {
        let info = t.await.expect("checkpoint task panicked");
        commit_ids.insert(info.commit_id);
    }
    assert_eq!(
        commit_ids.len(),
        N,
        "every concurrent checkpoint produced a distinct commit"
    );

    // The real proof: every writer's file is reachable through the
    // (single, linear) head — a lost commit would leave its file
    // either absent from `chain` or the file simply never having been
    // committed onto the line of history the tracked head walks.
    for i in 0..N {
        let chain = client
            .chain(root.id, format!("f{i}.txt"))
            .await
            .unwrap_or_else(|_| panic!("chain rpc for f{i}.txt"));
        assert_eq!(
            chain.len(),
            1,
            "f{i}.txt's checkpoint must not have been orphaned by a racing writer: {chain:?}"
        );
    }

    scope.close().await;
}

// A genuine two-process restart reopens in well under a second (per
// PR #280 review, refuting this test's earlier "reopen hangs" theory —
// the hang was two `FilesBackend`s alive in *one* process at once,
// this test's own earlier setup, most likely iroh-blobs' `FsStore`
// holding file-backed resources open until `ChunkStore::shutdown`).
// This version tears the first backend all the way down —
// `FilesBackend::shutdown` flushes its chunk stores, then every RPC
// handle/router/scope is dropped — before constructing the second.
#[tokio::test(flavor = "multi_thread")]
async fn root_identity_survives_backend_restart() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("session.rpp"), b"reaper project").unwrap();

    let created: FileRootInfo = {
        let backend =
            FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
        let scope = Scope::new();
        let local = LocalServer::serve(router(backend.clone()), scope.clone());
        let client: FilesServiceClient = local.establish().await.expect("establish client");
        let root = client
            .create_root(
                root_dir.to_str().unwrap().to_string(),
                "Session".to_string(),
                RootFlavor::Media,
            )
            .await
            .expect("create_root rpc");
        client
            .checkpoint_now(root.id, Some("initial".to_string()))
            .await
            .expect("checkpoint_now rpc");

        // Tear this backend all the way down before the second one
        // touches the same repo: flush its chunk stores, drop the RPC
        // client and the server/scope, then drop the backend itself.
        backend.shutdown().await;
        drop(client);
        scope.close().await;
        drop(local);
        drop(backend);
        // Give any detached background task (the chunk store's own
        // actor, if `shutdown` didn't fully join it) a beat to
        // actually exit before the second backend opens the same
        // on-disk store.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        root
    };

    // A fresh `FilesBackend` pointed at the same data dir (simulating a
    // server restart) still knows the root and can still derive its
    // chain — both the marker file and the registry entity, plus the
    // reopened jj repo, survive.
    let restart = tokio::time::timeout(Duration::from_secs(15), async {
        let backend =
            FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
        let scope = Scope::new();
        let local = LocalServer::serve(router(backend), scope.clone());
        let client: FilesServiceClient = local.establish().await.expect("establish client");

        let roots = client.list_roots().await.expect("list_roots rpc");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, created.id);

        let chain = client
            .chain(created.id, "session.rpp".to_string())
            .await
            .expect("chain rpc");
        assert_eq!(
            chain.len(),
            1,
            "the checkpoint from before the restart is still there"
        );

        scope.close().await;
    })
    .await;
    restart.expect("reopening the root after a full backend teardown must not hang");
}

/// Issue #266's badge data, at the RPC seam the spec names as primary.
///
/// A path the checkpoint head tracks but that is no longer resident in
/// the live tree comes back as a **pointer stub** — the explorer's
/// resident-vs-stub state — while resident entries stay `stub: false`.
#[tokio::test(flavor = "multi_thread")]
async fn browse_reports_pointer_stubs_for_non_resident_paths() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("video-cut");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("cut.mov"), b"a very large file").unwrap();
    std::fs::write(root_dir.join("notes.txt"), b"resident").unwrap();
    std::fs::create_dir(root_dir.join("media")).unwrap();
    std::fs::write(root_dir.join("media").join("b-roll.mov"), b"more").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Video Cut".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");
    client
        .checkpoint_now(root.id, Some("ingest".to_string()))
        .await
        .expect("checkpoint_now rpc");

    // Everything is resident right after the checkpoint.
    let top = client
        .browse(root.id, String::new())
        .await
        .expect("browse rpc");
    assert!(
        top.iter().all(|e| !e.stub),
        "a fully hydrated tree has no stubs: {top:?}"
    );

    // Drop the big file and a whole subtree from the live tree without
    // checkpointing — exactly what selective sync leaves behind.
    std::fs::remove_file(root_dir.join("cut.mov")).unwrap();
    std::fs::remove_dir_all(root_dir.join("media")).unwrap();

    let top = client
        .browse(root.id, String::new())
        .await
        .expect("browse rpc");
    let cut = top
        .iter()
        .find(|e| e.name == "cut.mov")
        .expect("a non-resident tracked file still appears in the listing");
    assert!(cut.stub, "tracked but not on disk = pointer stub: {cut:?}");
    assert!(!cut.is_dir);
    assert_eq!(cut.size, None, "a stub's size is not a live-tree size");
    let media = top
        .iter()
        .find(|e| e.name == "media")
        .expect("a non-resident tracked directory appears too");
    assert!(media.stub && media.is_dir, "{media:?}");
    let notes = top
        .iter()
        .find(|e| e.name == "notes.txt")
        .expect("the resident file is still listed");
    assert!(!notes.stub, "a resident file is never a stub: {notes:?}");
    assert_eq!(notes.size, Some(8));
    // Untracked-and-unresident is nothing at all.
    assert!(top.iter().all(|e| e.name != "never-existed"));

    // A directory that is entirely non-resident is still browsable —
    // its content answers from the store, as stubs.
    let inside = client
        .browse(root.id, "media".to_string())
        .await
        .expect("browsing a non-resident tracked directory");
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].name, "b-roll.mov");
    assert!(inside[0].stub, "{inside:?}");

    // A path that is neither on disk nor in the store is still a miss,
    // and an escaping subpath is still refused.
    assert!(client.browse(root.id, "nope".to_string()).await.is_err());
    assert!(client.browse(root.id, "../..".to_string()).await.is_err());

    // Drive browsing has no root context, so it never claims stubs.
    let drive = client
        .drive_browse(root_dir.to_str().unwrap().to_string())
        .await
        .expect("drive_browse rpc");
    assert!(
        drive.iter().all(|e| !e.stub && !e.divergent),
        "Drive browsing reports the raw tree only: {drive:?}"
    );

    scope.close().await;
}

/// Divergent versions — two saves from the same base, as an offline
/// replica reconciling would leave them — surface on the listing as the
/// explorer's divergence badge, derived from the store's visible heads
/// (never a second authority). Covers three things the badge exists
/// for:
///
/// 1. a file BOTH heads carry with different content is divergent;
/// 2. a file only the *other* head carries, never hydrated here, still
///    gets a row (stub + divergent) — a concurrent save nobody can see
///    is a save the user concludes was lost (PR #288 review);
/// 3. the answer arrives on a WARM backend — one that had already
///    browsed this root before the divergent writes — because a read
///    re-reads the op log rather than serving its cached snapshot.
///
/// The two saves are written through the backend's own cached repo
/// handle without touching its head cache, which is exactly what a
/// second process (the CLI's embedded backend, the cadence engine)
/// does to this one. Two `FilesBackend`s over one root cannot stand in:
/// they would share the root's single-writer chunk store.
#[tokio::test(flavor = "multi_thread")]
async fn browse_reports_divergent_versions_from_concurrent_saves() {
    use jj_lib::repo_path::RepoPathBuf;
    use files_store::version::checkpoint::{Change, checkpoint};

    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("split-session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("mix.wav"), b"base take").unwrap();
    std::fs::write(root_dir.join("readme.txt"), b"untouched").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Split Session".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");
    client
        .checkpoint_now(root.id, Some("base".to_string()))
        .await
        .expect("checkpoint_now rpc");

    // Warm the backend: this browse caches the repo handle at the base
    // checkpoint, so everything below has to survive a stale cache.
    let before = client
        .browse(root.id, String::new())
        .await
        .expect("browse rpc");
    assert!(
        before.iter().all(|e| !e.divergent),
        "nothing is divergent yet: {before:?}"
    );

    // Two saves from the same base, neither aware of the other. Side B
    // also ADDS a file that never lands in this machine's live tree —
    // the replica case the listing has to surface.
    let writer = backend.clone();
    tokio::task::spawn_blocking(move || {
        writer
            .with_repo(root.id, |repo| {
                let base = repo
                    .view()
                    .heads()
                    .iter()
                    .next()
                    .cloned()
                    .expect("base checkpoint head");
                let mix = RepoPathBuf::from_internal_string("mix.wav").unwrap();
                let solo = RepoPathBuf::from_internal_string("solo-from-b.wav").unwrap();
                pollster::block_on(checkpoint(
                    repo,
                    base.clone(),
                    vec![Change::Write {
                        path: mix.clone(),
                        content: b"take A: brighter".to_vec(),
                    }],
                    "side A",
                ))
                .expect("side A");
                pollster::block_on(checkpoint(
                    repo,
                    base,
                    vec![
                        Change::Write {
                            path: mix,
                            content: b"take B: warmer".to_vec(),
                        },
                        Change::Write {
                            path: solo,
                            content: b"a take only B has".to_vec(),
                        },
                    ],
                    "side B",
                ))
                .expect("side B");
            })
            .expect("with_repo");
    })
    .await
    .expect("divergent saves");

    // Same, already-warm backend: the read re-reads the op log, jj
    // merges the heads, and both saves are visible.
    let top = tokio::time::timeout(
        Duration::from_secs(30),
        client.browse(root.id, String::new()),
    )
    .await
    .expect("browse must not hang on a divergent root")
    .expect("browse rpc");

    let mix = top
        .iter()
        .find(|e| e.name == "mix.wav")
        .expect("mix.wav listed");
    assert!(
        mix.divergent,
        "two saves of one file from one base are divergent: {top:?}"
    );
    assert!(!mix.stub, "mix.wav is still resident on disk: {mix:?}");

    // The file only the other head has: a row, badged both ways.
    let solo = top
        .iter()
        .find(|e| e.name == "solo-from-b.wav")
        .unwrap_or_else(|| {
            panic!("a save only the other head carries must still be listed: {top:?}")
        });
    assert!(solo.stub, "not resident here, so it is a stub: {solo:?}");
    assert!(
        solo.divergent,
        "present on one head and not the other is divergence: {solo:?}"
    );

    let readme = top
        .iter()
        .find(|e| e.name == "readme.txt")
        .expect("readme.txt listed");
    assert!(
        !readme.divergent && !readme.stub,
        "a file both sides agree on is untouched: {readme:?}"
    );

    backend.shutdown().await;
    scope.close().await;
}

/// A root's lineage badge: `list_roots`/`get_root` project the root's
/// CURRENT Project Version — the highest-numbered entity (#261) — so
/// the explorer can render it without a second round trip, and a root
/// that has never been restarted carries none.
#[tokio::test(flavor = "multi_thread")]
async fn root_reads_carry_the_current_project_version() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("album");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("song.rpp"), b"session").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Album".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");
    assert_eq!(
        root.project_version, None,
        "a root that has never been restarted wears no badge"
    );
    client
        .checkpoint_now(root.id, Some("base".to_string()))
        .await
        .expect("checkpoint_now rpc");

    // A root with only v1 recorded already shows it — the badge is
    // "which lineage is this", not "has it been restarted twice".
    let v1 = client
        .start_project_version(root.id, None)
        .await
        .expect("start_project_version rpc");
    assert_eq!(v1.number, 1);
    let got = client.get_root(root.id).await.expect("get_root rpc");
    assert_eq!(got.project_version.as_ref().map(|pv| pv.number), Some(1));

    // A second restart wins: the badge is the CURRENT lineage.
    let v2 = client
        .start_project_version(root.id, Some("client cut".to_string()))
        .await
        .expect("start_project_version rpc");
    assert_eq!(v2.number, 2);

    let got = client.get_root(root.id).await.expect("get_root rpc");
    let badge = got.project_version.expect("badge on get_root");
    assert_eq!(badge.number, 2);
    assert_eq!(badge.label.as_deref(), Some("client cut"));

    let listed = client.list_roots().await.expect("list_roots rpc");
    assert_eq!(
        listed[0].project_version.as_ref().map(|pv| pv.number),
        Some(2),
        "list_roots carries the badge too — the explorer's root list reads it"
    );

    backend.shutdown().await;
    scope.close().await;
}

/// Browsing must never *write*. A registered root whose tree is gone
/// (an unmounted volume, a folder deleted behind Files' back) is a read
/// error — not a silent re-init of an empty version store inside the
/// stale mountpoint (PR #288 review).
#[tokio::test(flavor = "multi_thread")]
async fn browse_never_initializes_a_missing_store() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("on-a-drive");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("take.wav"), b"take").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "On A Drive".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");
    backend.shutdown().await;

    // Simulate the volume going away: the tree (store included) is no
    // longer there, but the registry still knows the root.
    std::fs::remove_dir_all(&root_dir).unwrap();
    // …and the mountpoint itself still exists, empty — the shape that
    // used to get a store initialized into it.
    std::fs::create_dir(&root_dir).unwrap();

    let listed = client
        .browse(root.id, String::new())
        .await
        .expect("an empty mountpoint lists as empty, not as an error");
    assert!(listed.is_empty(), "nothing is there: {listed:?}");
    assert!(
        !root_dir.join(".fts-files").exists(),
        "a read must not initialize a version store"
    );

    scope.close().await;
}

/// REPRO (divergent-root cold-open browse hang): a divergence written by
/// one process, then a *fresh* backend opens the root cold and browses.
/// The dev seed does exactly this (seed process → server process), and
/// the UI wedges. Asserts the cold-open browse completes.
#[tokio::test(flavor = "multi_thread")]
async fn cold_open_browse_on_a_divergent_root_completes() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("split");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("mix.wav"), b"base take").unwrap();

    // Process 1: create + checkpoint + seed a divergence, then shut down.
    {
        let backend =
            FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
        let scope = Scope::new();
        let local = LocalServer::serve(router(backend.clone()), scope.clone());
        let client: FilesServiceClient = local.establish().await.expect("client");
        let root = client
            .create_root(
                root_dir.to_str().unwrap().to_string(),
                "Split".to_string(),
                RootFlavor::Media,
            )
            .await
            .expect("create_root");
        client
            .checkpoint_now(root.id, Some("base".to_string()))
            .await
            .expect("checkpoint");
        backend
            .seed_divergent_file(root.id, "mix.wav", b"take A", b"take B")
            .await
            .expect("seed divergence");
        backend.shutdown().await;
        scope.close().await;
    }

    // Process 2: a cold backend opens the same store and browses —
    // faithfully to the server, with cadence watching + driver running.
    let backend2 =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend2");
    backend2.enable_watching().await;
    backend2.spawn_cadence_driver(Duration::from_secs(30));
    let scope2 = Scope::new();
    let local2 = LocalServer::serve(router(backend2.clone()), scope2.clone());
    let client2: FilesServiceClient = local2.establish().await.expect("client2");
    let roots = client2.list_roots().await.expect("list_roots");
    let root_id = roots.first().expect("a root").id;

    let browsed = tokio::time::timeout(
        Duration::from_secs(20),
        client2.browse(root_id, String::new()),
    )
    .await;
    assert!(
        browsed.is_ok(),
        "cold-open browse on a divergent root hung (>20s)"
    );
    let entries = browsed.unwrap().expect("browse rpc");
    assert!(
        entries.iter().any(|e| e.name == "mix.wav" && e.divergent),
        "mix.wav should be divergent: {entries:?}"
    );
    backend2.shutdown().await;
    scope2.close().await;
}
