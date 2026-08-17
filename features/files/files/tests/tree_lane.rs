//! `TreeService` — the namespace and the catalogue over it, against a
//! real `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, for the same reason
//! `roots_lane` is: the lane has no `permits.rs` rows yet, so it is not
//! mounted on a router and cannot be reached over vox. These call the
//! trait directly, which is what the dispatcher would do.

use files::FilesBackend;
use files_proto::id::RootId;
use files_proto::model::RootFlavor;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::tree::{Cursor, EntryKind, Hydration, TreeService};
use files_proto::{FilesFault, RootPath, TreePath};

/// A backend confined to its own data dir, holding one adopted root with
/// a nested tree.
async fn adopted() -> (tempfile::TempDir, FilesBackend, RootId) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();
    std::fs::create_dir(dir.join("stems").join("perc")).unwrap();
    std::fs::write(dir.join("stems").join("perc").join("shaker.wav"), b"ssh").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Mix Session".into(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");

    (data_dir, backend, RootId::new(root.id))
}

fn p(raw: &str) -> RootPath {
    RootPath::parse(raw).expect("a valid root path")
}

#[tokio::test(flavor = "multi_thread")]
async fn browse_lists_the_live_tree_one_level_at_a_time() {
    let (_tmp, backend, id) = adopted().await;

    let top = TreeService::browse(&backend, id, RootPath::root())
        .await
        .expect("browse root");
    let names: Vec<&str> = top.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"mix.wav"), "got {names:?}");
    assert!(names.contains(&"stems"), "got {names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with('.')),
        "a root's own bookkeeping is not its content: {names:?}"
    );

    let stems = TreeService::browse(&backend, id, p("stems"))
        .await
        .expect("browse stems");
    assert!(stems.iter().any(|e| e.name == "kick.wav" && !e.is_dir));
    assert!(stems.iter().any(|e| e.name == "perc" && e.is_dir));
}

#[tokio::test(flavor = "multi_thread")]
async fn browsing_an_unknown_root_names_the_root_it_could_not_find() {
    let (_tmp, backend, _) = adopted().await;
    let ghost = RootId::generate();
    match TreeService::browse(&backend, ghost, RootPath::root())
        .await
        .expect_err("no such root")
    {
        FilesFault::RootNotFound(got) => assert_eq!(got, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }
}

/// The typed path is checked once, at the edge, rather than once per
/// method — and re-checked here because the type is transparent on the
/// wire.
#[tokio::test(flavor = "multi_thread")]
async fn browse_refuses_a_path_that_escapes_its_root() {
    let (_tmp, backend, id) = adopted().await;
    // Bypass `parse` exactly as a hostile peer's `Deserialize` would.
    let hostile: RootPath = serde_json::from_str("\"../..\"").expect("transparent decode");
    assert!(
        matches!(
            TreeService::browse(&backend, id, hostile).await,
            Err(FilesFault::BadPath(_))
        ),
        "a path that never saw `parse` is still refused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn resolve_lists_the_org_tree_areas_at_the_top() {
    let (_tmp, backend, _) = adopted().await;
    let node = TreeService::resolve(&backend, TreePath::root())
        .await
        .expect("resolve the areas");
    let files_proto::TreeNode::Listing(areas) = node else {
        panic!("the top of the org tree is a listing of areas");
    };
    let names: Vec<&str> = areas.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Projects"), "got {names:?}");
    assert!(names.contains(&"Vault"), "got {names:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn resolving_a_path_outside_the_namespace_is_a_typed_fault() {
    let (_tmp, backend, _) = adopted().await;
    let path = TreePath::parse("Nowhere/At/All").unwrap();
    match TreeService::resolve(&backend, path.clone()).await {
        Err(FilesFault::TreePathNotFound(got)) => assert_eq!(got, path),
        Err(FilesFault::Invalid(_)) => {} // the grammar rejects the area outright
        other => panic!("expected a typed miss, got {other:?}"),
    }
}

// t[verify files.catalogue.complete]
#[tokio::test(flavor = "multi_thread")]
async fn the_catalogue_covers_every_reachable_path_at_every_depth() {
    let (_tmp, backend, id) = adopted().await;

    let delta = backend.catalogue(id, None).await.expect("catalogue");
    let paths: Vec<&str> = delta.changed.iter().map(|e| e.path.as_str()).collect();

    for expected in [
        "mix.wav",
        "stems",
        "stems/kick.wav",
        "stems/perc",
        "stems/perc/shaker.wav",
    ] {
        assert!(
            paths.contains(&expected),
            "{expected} missing from {paths:?}"
        );
    }
    assert!(!delta.more, "this tree fits in one page");

    let shaker = delta
        .changed
        .iter()
        .find(|e| e.path.as_str() == "stems/perc/shaker.wav")
        .unwrap();
    assert_eq!(shaker.kind, EntryKind::File);
    assert_eq!(shaker.size, 3);
    assert_eq!(shaker.root_id, id);
}

// t[verify files.catalogue.complete]
#[tokio::test(flavor = "multi_thread")]
async fn entry_answers_for_one_path_without_listing_its_parent() {
    let (_tmp, backend, id) = adopted().await;

    let e = backend.entry(id, p("stems/kick.wav")).await.expect("entry");
    assert_eq!(e.path, p("stems/kick.wav"));
    assert_eq!(e.size, 4);
    assert_eq!(e.hydration, Hydration::Resident);
    assert_eq!(
        e.content, None,
        "published before it is verified — adoption's tail, not a fabricated address"
    );

    match backend.entry(id, p("stems/ghost.wav")).await {
        Err(FilesFault::PathNotFound(got)) => assert_eq!(got, p("stems/ghost.wav")),
        other => panic!("expected PathNotFound, got {other:?}"),
    }
}

/// The rule the whole catalogue exists to hold: content being out of
/// reach changes an entry's hydration, never its existence. A folder that
/// cannot be read must still be *there*, marked — because a folder that
/// silently lists empty is indistinguishable from a folder someone
/// deleted, and one of those is a data-loss report.
// t[verify files.catalogue.offline]
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_folder_is_present_and_marked_never_missing() {
    use std::os::unix::fs::PermissionsExt as _;

    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join("archive");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("readme.txt"), b"here").unwrap();
    let sealed = dir.join("vault");
    std::fs::create_dir(&sealed).unwrap();
    std::fs::write(sealed.join("master.wav"), b"the masters").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault-store")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Archive".into(),
            flavor: RootFlavor::Media,
            hash_content: false,
        })
        .await
        .expect("adopt");
    let id = RootId::new(root.id);

    // Make the folder unlistable — the local stand-in for a location
    // that has stopped answering.
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();

    let delta = backend.catalogue(id, None).await.expect("catalogue");
    let entry = backend.entry(id, p("vault")).await;
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();

    let entry = entry.expect("the folder is still in the tree; only its content is out of reach");
    assert_eq!(entry.kind, EntryKind::Directory);
    assert_eq!(
        entry.hydration,
        Hydration::Unavailable,
        "unreachable is a marking, not a deletion"
    );
    assert!(
        delta.changed.iter().any(|e| e.path.as_str() == "vault"),
        "and it is in the listing a client syncs, not only in a point lookup"
    );
    assert!(
        delta
            .changed
            .iter()
            .any(|e| e.path.as_str() == "readme.txt"),
        "one unreadable folder does not take the rest of the listing with it"
    );
    // The delta replays the log, so `vault` appears twice — first as the
    // parent's listing found it, then re-marked. Last-wins is the
    // contract a client folds by; asserting it here is what stops a
    // future paging change from reordering the log.
    let states: Vec<_> = delta
        .changed
        .iter()
        .filter(|e| e.path.as_str() == "vault")
        .map(|e| e.hydration)
        .collect();
    assert_eq!(
        states.last(),
        Some(&Hydration::Unavailable),
        "the last state for a path is the current one: {states:?}"
    );
}

// t[verify files.catalogue.concurrent]
#[tokio::test(flavor = "multi_thread")]
async fn a_client_resumes_from_its_cursor_rather_than_re_listing() {
    let (_tmp, backend, id) = adopted().await;

    let first = backend.catalogue(id, None).await.expect("initial sync");
    assert!(!first.changed.is_empty());

    let caught_up = backend
        .changes_since(id, first.cursor.clone())
        .await
        .expect("changes since");
    assert!(
        caught_up.changed.is_empty() && caught_up.removed.is_empty(),
        "a client that is current is told nothing, not told everything again"
    );
    assert!(!caught_up.more);
    assert_eq!(
        caught_up.cursor, first.cursor,
        "and its position does not drift when nothing happened"
    );
}

// t[verify files.catalogue.concurrent]
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_this_process_cannot_place_resyncs_rather_than_skipping() {
    let (_tmp, backend, id) = adopted().await;
    let full = backend.catalogue(id, None).await.expect("initial sync");

    // What a client holding a cursor from before a restart sends.
    let stale = backend
        .changes_since(id, Cursor("from-a-previous-life".into()))
        .await
        .expect("changes since");
    assert_eq!(
        stale.changed.len(),
        full.changed.len(),
        "the safe direction is to resend, never to skip changes it never saw"
    );
}

// t[verify files.catalogue.staleness]
#[tokio::test(flavor = "multi_thread")]
async fn freshness_says_as_of_when_and_for_which_root() {
    let (_tmp, backend, id) = adopted().await;

    let all = backend.freshness().await.expect("freshness");
    let f = all
        .iter()
        .find(|f| f.root_id == id)
        .expect("every registered root reports its currency");
    assert!(f.reachable, "we just read this tree");
    assert_eq!(f.entries, 5, "and it says how much it is speaking for");
    assert!(f.confirmed_at <= chrono::Utc::now());
}

#[tokio::test(flavor = "multi_thread")]
async fn catalogue_questions_about_an_unknown_root_are_typed_faults() {
    let (_tmp, backend, _) = adopted().await;
    let ghost = RootId::generate();

    assert!(matches!(
        backend.catalogue(ghost, None).await.expect_err("no root"),
        FilesFault::RootNotFound(_)
    ));
    assert!(matches!(
        backend.entry(ghost, p("a")).await.expect_err("no root"),
        FilesFault::RootNotFound(_)
    ));
    assert!(matches!(
        backend
            .changes_since(ghost, Cursor("0".into()))
            .await
            .expect_err("no root"),
        FilesFault::RootNotFound(_)
    ));
}
