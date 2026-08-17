//! `OrganiseService` against a real `FilesBackend` and a real root on
//! disk.
//!
//! In-process rather than over `LocalServer`, for the reason
//! `roots_lane.rs` gives: the lane has no `permits.rs` rows yet, so it is
//! not mounted on a router. These call the trait directly, which is what
//! the dispatcher would do.
//!
//! The load-bearing test here is
//! [`tagging_produces_a_view_and_moves_nothing`] — the whole of
//! `files.organise.manual` is that a tag is not a folder, and the only
//! way to show it is to photograph the tree before and after.

use std::collections::BTreeSet;

use files::FilesBackend;
use files_proto::id::RootId;
use files_proto::model::RootFlavor;
use files_proto::service::legacy::FilesService;
use files_proto::service::organise::{Action, OrganiseService, Tag};
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::{FilesFault, RootPath};

/// A backend confined to its own data dir, with one staged folder ready
/// to adopt.
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

async fn adopted(name: &str) -> (tempfile::TempDir, FilesBackend, String, RootId) {
    let (tmp, backend, path) = staged(name);
    let root = backend
        .adopt(AdoptRequest {
            path: path.clone(),
            name: name.to_string(),
            flavor: RootFlavor::Media,
            hash_content: false,
        })
        .await
        .expect("adopt");
    (tmp, backend, path, RootId::new(root.id))
}

/// Every path under `dir`, relative and sorted — a photograph of the tree
/// good enough to prove nothing moved.
fn tree(dir: &str) -> BTreeSet<String> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            out.insert(
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let base = std::path::Path::new(dir);
    let mut out = BTreeSet::new();
    walk(base, base, &mut out);
    out
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("path")
}

fn tags(names: &[&str]) -> Vec<Tag> {
    names.iter().map(|n| Tag((*n).to_string())).collect()
}

/// The rule, whole: a tag produces a view, never folder membership.
// t[verify files.organise.manual]
#[tokio::test(flavor = "multi_thread")]
async fn tagging_produces_a_view_and_moves_nothing() {
    let (_tmp, backend, path, root) = adopted("mix-session").await;
    let before = tree(&path);
    let bytes_before = std::fs::read(std::path::Path::new(&path).join("mix.wav")).unwrap();

    let marks = backend
        .set_tags(root, p("mix.wav"), tags(&["client edit", "Keep"]))
        .await
        .expect("set_tags");

    assert_eq!(
        marks.path,
        p("mix.wav"),
        "the path that went in is the path that comes back — tagging does not relocate"
    );
    assert_eq!(
        tree(&path),
        before,
        "tagging created no folder, moved nothing and renamed nothing"
    );
    assert_eq!(
        std::fs::read(std::path::Path::new(&path).join("mix.wav")).unwrap(),
        bytes_before,
        "and did not touch content"
    );
    assert!(
        !std::path::Path::new(&path).join("client edit").exists(),
        "a tag is not a directory"
    );

    // The view the tag produces is a query, not a listing.
    let view = backend
        .tagged(tags(&["client edit"]), Some(root))
        .await
        .expect("tagged");
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].path, p("mix.wav"));
    assert_eq!(view[0].root_id, root);
}

// t[verify files.organise.manual]
#[tokio::test(flavor = "multi_thread")]
async fn tags_are_replaced_wholesale_and_read_back_folded() {
    let (_tmp, backend, _, root) = adopted("takes").await;

    backend
        .set_tags(
            root,
            p("stems/kick.wav"),
            tags(&["Drums", " drums ", "raw"]),
        )
        .await
        .expect("set_tags");
    let marks = backend
        .marks(root, p("stems/kick.wav"))
        .await
        .expect("marks");
    assert_eq!(
        marks.tags,
        tags(&["drums", "raw"]),
        "one label however it was capitalised or padded"
    );

    // `set_tags` replaces: this is an edit of the whole set, not an add.
    backend
        .set_tags(root, p("stems/kick.wav"), tags(&["raw"]))
        .await
        .expect("set_tags");
    assert_eq!(
        backend.marks(root, p("stems/kick.wav")).await.unwrap().tags,
        tags(&["raw"])
    );

    // And an empty set clears the mark rather than leaving a ghost.
    backend
        .set_tags(root, p("stems/kick.wav"), Vec::new())
        .await
        .expect("set_tags");
    assert!(
        backend
            .marks(root, p("stems/kick.wav"))
            .await
            .unwrap()
            .tags
            .is_empty()
    );
    assert!(
        backend
            .tagged(tags(&["raw"]), Some(root))
            .await
            .unwrap()
            .is_empty()
    );
}

// t[verify files.organise.manual]
#[tokio::test(flavor = "multi_thread")]
async fn two_tags_narrow_the_view_and_all_tags_reports_what_exists() {
    let (_tmp, backend, _, root) = adopted("album").await;
    backend
        .set_tags(root, p("mix.wav"), tags(&["keep", "client"]))
        .await
        .unwrap();
    backend
        .set_tags(root, p("stems/kick.wav"), tags(&["keep"]))
        .await
        .unwrap();

    let both = backend
        .tagged(tags(&["keep", "client"]), Some(root))
        .await
        .unwrap();
    assert_eq!(
        both.iter().map(|m| m.path.clone()).collect::<Vec<_>>(),
        vec![p("mix.wav")],
        "naming two tags narrows the view rather than widening it"
    );
    assert_eq!(
        backend
            .tagged(tags(&["keep"]), Some(root))
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        backend.all_tags(Some(root)).await.unwrap(),
        tags(&["client", "keep"]),
        "completion sees every tag in use on the root, sorted"
    );

    let ghost = RootId::generate();
    assert!(matches!(
        backend
            .all_tags(Some(ghost))
            .await
            .expect_err("no such root"),
        FilesFault::RootNotFound(_)
    ));
    assert!(
        backend.tagged(tags(&["keep"]), Some(ghost)).await.is_err(),
        "a view is scoped to a root that exists"
    );
}

/// A favourite is the caller's own shortlist and is not a tag — marking
/// one must not make the file show up in anybody's tag view.
// t[verify files.organise.manual]
#[tokio::test(flavor = "multi_thread")]
async fn a_favourite_is_the_callers_own_and_is_not_a_tag() {
    let (_tmp, backend, path, root) = adopted("favourites").await;
    let before = tree(&path);

    let marks = backend
        .set_favourite(root, p("mix.wav"), true)
        .await
        .expect("set_favourite");
    assert!(marks.favourite);
    assert!(marks.tags.is_empty(), "a favourite is not a tag");
    assert_eq!(tree(&path), before, "and moves nothing either");
    assert!(
        backend.all_tags(Some(root)).await.unwrap().is_empty(),
        "and does not appear in the tag vocabulary"
    );

    assert!(backend.marks(root, p("mix.wav")).await.unwrap().favourite);
    assert!(
        !backend
            .marks(root, p("stems/kick.wav"))
            .await
            .unwrap()
            .favourite,
        "a shortlist holds what was put on it and nothing else"
    );

    let cleared = backend
        .set_favourite(root, p("mix.wav"), false)
        .await
        .unwrap();
    assert!(!cleared.favourite);
}

/// Marks are independent of extracted metadata and of the tree: a path
/// with no resident bytes is still organisable, and an unknown root is
/// still a typed fault.
// t[verify files.organise.manual]
#[tokio::test(flavor = "multi_thread")]
async fn marks_address_paths_and_roots_typed() {
    let (_tmp, backend, _, root) = adopted("typed").await;

    let unmarked = backend.marks(root, p("stems")).await.expect("marks");
    assert!(unmarked.tags.is_empty() && !unmarked.favourite);
    assert_eq!(unmarked.root_id, root);

    match backend
        .marks(RootId::generate(), p("mix.wav"))
        .await
        .expect_err("no such root")
    {
        FilesFault::RootNotFound(_) => {}
        other => panic!("expected RootNotFound, got {other:?}"),
    }
    assert!(
        matches!(
            backend
                .set_tags(root, p("mix.wav"), tags(&["  "]))
                .await
                .expect_err("blank tag"),
            FilesFault::Invalid(_)
        ),
        "whitespace is not a label"
    );
}

/// The feed is the recorded stream, read back — a root that exists has a
/// creation row, and a root that has checkpointed has the checkpoint.
// t[verify files.organise.activity]
#[tokio::test(flavor = "multi_thread")]
async fn every_structural_change_shows_up_in_the_feed() {
    let (_tmp, backend, _, root) = adopted("accountable").await;

    let created = backend.activity(root, None, None).await.expect("activity");
    assert_eq!(
        created.len(),
        1,
        "a fresh root has been created and nothing else"
    );
    assert_eq!(created[0].action, Action::Created);
    assert!(created[0].path.is_root());
    assert_eq!(created[0].root_id, root);

    FilesService::checkpoint_now(&backend, root.get(), Some("first pass".into()))
        .await
        .expect("checkpoint");

    let after = backend.activity(root, None, None).await.expect("activity");
    assert!(
        after.len() > created.len(),
        "the checkpoint that propagated is the checkpoint that is recorded: {after:?}"
    );
    assert_eq!(after[0].action, Action::Modified);
    assert!(after[0].at >= after[after.len() - 1].at, "newest first");
    assert_eq!(
        after[after.len() - 1].action,
        Action::Created,
        "and the creation is still the oldest row"
    );

    // Scoping: a subtree that nothing has touched has no rows, and the
    // root's own rows are not smuggled into a file's feed.
    assert!(
        backend
            .activity(root, Some(p("stems/kick.wav")), None)
            .await
            .expect("scoped")
            .iter()
            .all(|a| a.path.is_within(&p("stems/kick.wav"))),
        "a file's feed holds that file's rows"
    );
    assert_eq!(
        backend.activity(root, None, Some(1)).await.unwrap().len(),
        1,
        "a limit keeps the newest"
    );
}

/// Tagging is not a structural change, and this lane keeps no log of its
/// own — the decision the rule forces, pinned.
// t[verify files.organise.activity]
#[tokio::test(flavor = "multi_thread")]
async fn organising_writes_no_activity_of_its_own() {
    let (_tmp, backend, _, root) = adopted("no-log").await;
    let before = backend.activity(root, None, None).await.unwrap();

    backend
        .set_tags(root, p("mix.wav"), tags(&["keep"]))
        .await
        .unwrap();
    backend
        .set_favourite(root, p("mix.wav"), true)
        .await
        .unwrap();

    assert_eq!(
        backend.activity(root, None, None).await.unwrap(),
        before,
        "a mark moves no bytes, so there is nothing structural to record — \
         and a row written here would be a second log, which is what the rule forbids"
    );
}

// t[verify files.organise.activity]
#[tokio::test(flavor = "multi_thread")]
async fn a_feed_is_scoped_to_a_root_that_exists() {
    let (_tmp, backend, _, _root) = adopted("scoped").await;
    assert!(matches!(
        backend
            .activity(RootId::generate(), None, None)
            .await
            .expect_err("no such root"),
        FilesFault::RootNotFound(_)
    ));
}

/// Tags survive a restart.
///
/// `storage.tier.authored` puts anything a human chose in durable
/// storage; a tag is chosen. Before this, tags died with the process —
/// which meant the feature demonstrated and did not work.
#[tokio::test(flavor = "multi_thread")]
async fn tags_survive_a_restart() {
    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("album");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("take.wav"), b"one").unwrap();

    let adopt = |backend: &FilesBackend| {
        let path = dir.to_string_lossy().into_owned();
        let backend = backend.clone();
        async move {
            backend
                .adopt(AdoptRequest {
                    path,
                    name: "Album".into(),
                    flavor: RootFlavor::Media,
                    hash_content: true,
                })
                .await
        }
    };

    let take = RootPath::parse("take.wav").unwrap();
    let root = {
        let first = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
        let root = RootId::new(adopt(&first).await.expect("adopt").id);
        first
            .set_tags(root, take.clone(), vec![Tag("keeper".into())])
            .await
            .expect("tag");
        root
    };

    // The state file is the proof. A second backend in THIS process
    // shares the same in-memory cache, so reading through it would
    // answer from memory and say nothing about durability — which is
    // what the first version of this test did.
    let on_disk = std::fs::read_to_string(tmp.path().join("organise.json"))
        .expect("the lane must have written its state to the org's data dir");
    assert!(
        on_disk.contains("keeper"),
        "a tag a human chose must reach the disk, not just the cache: {on_disk}"
    );

    // And it still reads back through the lane.
    let second = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("restart");
    let marks = second.marks(root, take).await.expect("marks after restart");
    assert!(marks.tags.iter().any(|t| t.0 == "keeper"));
}

/// One org's tags are not another's.
///
/// The lane state used to be a process-wide `static`, but `FilesBackend`
/// is constructed once per org — so `all_tags` was answering with every
/// org's vocabulary on the server.
#[tokio::test(flavor = "multi_thread")]
async fn one_orgs_tags_are_not_anothers() {
    async fn org(name: &str) -> (tempfile::TempDir, FilesBackend, RootId, RootPath) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("take.wav"), b"one").unwrap();
        let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
        let root = backend
            .adopt(AdoptRequest {
                path: dir.to_string_lossy().into_owned(),
                name: name.into(),
                flavor: RootFlavor::Media,
                hash_content: true,
            })
            .await
            .expect("adopt");
        (
            tmp,
            backend,
            RootId::new(root.id),
            RootPath::parse("take.wav").unwrap(),
        )
    }

    let (_a_tmp, a, a_root, a_path) = org("alpha").await;
    let (_b_tmp, b, b_root, b_path) = org("beta").await;

    a.set_tags(a_root, a_path, vec![Tag("alpha-only".into())])
        .await
        .expect("tag a");
    b.set_tags(b_root, b_path, vec![Tag("beta-only".into())])
        .await
        .expect("tag b");

    let a_vocab = a.all_tags(None).await.expect("a vocabulary");
    let b_vocab = b.all_tags(None).await.expect("b vocabulary");

    assert!(a_vocab.iter().any(|t| t.0 == "alpha-only"));
    assert!(
        !a_vocab.iter().any(|t| t.0 == "beta-only"),
        "one org must not see another's tag vocabulary: {a_vocab:?}"
    );
    assert!(!b_vocab.iter().any(|t| t.0 == "alpha-only"));
}
