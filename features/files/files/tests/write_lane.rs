//! `WriteService` — the lane that makes the tree mutable over the wire.
//!
//! In-process against a real `FilesBackend`, like `roots_lane`: the lane
//! has no `permits.rs` row yet, so it is not mounted on a router and
//! cannot be reached over vox. These call the trait exactly as the
//! dispatcher would.
//!
//! Every test here adopts a real folder, writes through the lane and then
//! looks at the *filesystem*, because the whole claim of this lane is
//! about what happens to bytes on disk — asserting only on the receipt
//! would pass just as happily against a lane that did nothing.

use std::path::{Path, PathBuf};

use files::{FilesBackend, FilesService};
use files_proto::id::RootId;
use files_proto::model::RootFlavor;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::write::{OnConflict, Relocation, WriteService};
use files_proto::{FilesFault, RootPath};

/// A backend confined to its own data dir, with one adopted root:
///
/// ```text
/// mix.wav
/// stems/kick.wav
/// stems/snare.wav
/// ```
async fn adopted() -> (tempfile::TempDir, FilesBackend, RootId, PathBuf) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join("session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();
    std::fs::write(dir.join("stems").join("snare.wav"), b"crack").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Session".into(),
            flavor: RootFlavor::Media,
            hash_content: false,
        })
        .await
        .expect("adopt");

    (data_dir, backend, RootId::new(root.id), dir)
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

fn moves(pairs: &[(&str, &str)]) -> Vec<Relocation> {
    pairs
        .iter()
        .map(|(from, to)| Relocation {
            from: p(from),
            to: p(to),
        })
        .collect()
}

/// Every entry under `dir`, relative and sorted — the shape of the tree,
/// with the version store's own directory left out.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == files::STORE_DIR || name == files::MARKER_FILE {
                continue;
            }
            out.push(
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            if path.is_dir() {
                walk(&path, base, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

// ── Reachability ────────────────────────────────────────────────────

/// The headline: mkdir, rename, move, copy and delete all exist and all
/// change the tree. v1 had none of them.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn every_structural_operation_is_reachable() {
    let (_tmp, backend, id, dir) = adopted().await;

    backend
        .create_dirs(id, vec![p("Renders/Final")])
        .await
        .expect("mkdir");
    assert!(dir.join("Renders/Final").is_dir());

    <FilesBackend as WriteService>::rename(&backend, id, p("mix.wav"), "master.wav".into())
        .await
        .expect("rename");
    assert!(dir.join("master.wav").exists());
    assert!(!dir.join("mix.wav").exists());

    backend
        .move_paths(
            id,
            moves(&[("master.wav", "Renders/Final/master.wav")]),
            OnConflict::Fail,
        )
        .await
        .expect("move");
    assert!(dir.join("Renders/Final/master.wav").exists());

    backend
        .copy_paths(
            id,
            moves(&[("stems/kick.wav", "Renders/kick.wav")]),
            OnConflict::Fail,
        )
        .await
        .expect("copy");
    assert_eq!(
        std::fs::read(dir.join("Renders/kick.wav")).unwrap(),
        b"boom"
    );
    assert!(
        dir.join("stems/kick.wav").exists(),
        "a copy leaves its source alone"
    );

    backend
        .delete_paths(id, vec![p("stems/snare.wav")])
        .await
        .expect("delete");
    assert!(!dir.join("stems/snare.wav").exists());
}

// ── One operation per action ────────────────────────────────────────

/// History has to show "the user moved three files", not three writes.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_becomes_exactly_one_version_store_operation() {
    let (_tmp, backend, id, _dir) = adopted().await;
    backend.create_dirs(id, vec![p("Takes")]).await.unwrap();

    let receipt = backend
        .move_paths(
            id,
            moves(&[
                ("mix.wav", "Takes/mix.wav"),
                ("stems/kick.wav", "Takes/kick.wav"),
                ("stems/snare.wav", "Takes/snare.wav"),
            ]),
            OnConflict::Fail,
        )
        .await
        .expect("move three");

    assert_eq!(receipt.outcomes.len(), 3, "one outcome per path");
    assert!(
        !receipt.operation.is_empty(),
        "the batch names the single operation it became"
    );
    // The three renames are one commit, so the receipt's operation is one
    // id rather than a set — the shape of the receipt is the guarantee.
    assert_eq!(receipt.root_id, id);
}

// ── Transactionality ────────────────────────────────────────────────

/// The set applies wholly or not at all: one bad member and the tree is
/// exactly as it was, including the members that would have succeeded.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_with_one_bad_member_leaves_the_tree_untouched() {
    let (_tmp, backend, id, dir) = adopted().await;
    backend.create_dirs(id, vec![p("Takes")]).await.unwrap();
    let before = tree(&dir);

    let err = backend
        .move_paths(
            id,
            moves(&[
                ("mix.wav", "Takes/mix.wav"),
                ("stems/ghost.wav", "Takes/ghost.wav"),
            ]),
            OnConflict::Fail,
        )
        .await
        .expect_err("one member does not exist");

    assert!(matches!(err, FilesFault::PathNotFound(_)));
    assert_eq!(
        tree(&dir),
        before,
        "the good member must not land when its batch fails"
    );
}

/// Two paths in one set aimed at the same destination is a contradiction,
/// not a collision — and it is caught before anything moves.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_cannot_land_two_paths_on_one_destination() {
    let (_tmp, backend, id, dir) = adopted().await;
    backend.create_dirs(id, vec![p("Takes")]).await.unwrap();
    let before = tree(&dir);

    let err = backend
        .move_paths(
            id,
            moves(&[
                ("stems/kick.wav", "Takes/one.wav"),
                ("stems/snare.wav", "Takes/one.wav"),
            ]),
            OnConflict::Fail,
        )
        .await
        .expect_err("two into one");

    assert!(matches!(err, FilesFault::Exists { .. }));
    assert_eq!(tree(&dir), before);
}

// ── Idempotence ─────────────────────────────────────────────────────

/// A client retrying after a dropped connection must not fail.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn create_dirs_is_idempotent() {
    let (_tmp, backend, id, dir) = adopted().await;

    let first = backend
        .create_dirs(id, vec![p("Renders/Final")])
        .await
        .expect("first");
    assert!(!first.outcomes[0].skipped);

    let second = backend
        .create_dirs(id, vec![p("Renders/Final")])
        .await
        .expect("a retry is not an error");
    assert!(
        second.outcomes[0].skipped,
        "and says it did nothing rather than pretending it made it"
    );
    assert!(dir.join("Renders/Final").is_dir());
}

/// Idempotence is about directories. A file sitting where a directory was
/// asked for is a genuine clash, and quietly accepting it would lose
/// content the caller never mentioned.
#[tokio::test(flavor = "multi_thread")]
async fn create_dirs_refuses_a_file_in_the_way() {
    let (_tmp, backend, id, _dir) = adopted().await;
    let err = backend
        .create_dirs(id, vec![p("mix.wav")])
        .await
        .expect_err("that is a file");
    assert!(matches!(err, FilesFault::NotADirectory(_)));
}

// ── OnConflict ──────────────────────────────────────────────────────

// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn fail_refuses_and_says_what_collided() {
    let (_tmp, backend, id, dir) = adopted().await;
    let before = tree(&dir);
    let err = backend
        .copy_paths(
            id,
            moves(&[("stems/kick.wav", "mix.wav")]),
            OnConflict::Fail,
        )
        .await
        .expect_err("occupied");
    match err {
        FilesFault::Exists { path } => assert_eq!(path, p("mix.wav")),
        other => panic!("expected Exists, got {other:?}"),
    }
    assert_eq!(tree(&dir), before);
}

// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn keep_both_derives_a_name_and_reports_where_it_landed() {
    let (_tmp, backend, id, dir) = adopted().await;
    let receipt = backend
        .copy_paths(
            id,
            moves(&[("stems/kick.wav", "mix.wav")]),
            OnConflict::KeepBoth,
        )
        .await
        .expect("keep both");

    assert_eq!(
        receipt.outcomes[0].landed_at,
        Some(p("mix (2).wav")),
        "the suffix goes before the extension, or the file changes type"
    );
    assert_eq!(std::fs::read(dir.join("mix.wav")).unwrap(), b"take one");
    assert_eq!(std::fs::read(dir.join("mix (2).wav")).unwrap(), b"boom");
}

/// Replace does not discard: the displaced content is in the checkpoint
/// this batch built on, and the batch records a new one over it.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn replace_takes_the_destination_and_records_a_new_version() {
    let (_tmp, backend, id, dir) = adopted().await;
    // A checkpoint first, so the displaced content is genuinely in
    // history rather than merely uncommitted.
    backend.checkpoint_now(id.get(), None).await.expect("base");

    let receipt = backend
        .copy_paths(
            id,
            moves(&[("stems/kick.wav", "mix.wav")]),
            OnConflict::Replace,
        )
        .await
        .expect("replace");

    assert_eq!(std::fs::read(dir.join("mix.wav")).unwrap(), b"boom");
    assert_eq!(receipt.outcomes[0].landed_at, Some(p("mix.wav")));
    assert!(
        !dir.join(files::STORE_DIR).join("write-staging").exists(),
        "the staging area does not outlive the batch"
    );

    let chain = backend
        .chain(id.get(), "mix.wav".into())
        .await
        .expect("history for the replaced path");
    assert!(
        chain.len() >= 2,
        "replace records a new version rather than discarding the old: {chain:?}"
    );
}

// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn keep_existing_leaves_the_occupant_alone_and_says_it_skipped() {
    let (_tmp, backend, id, dir) = adopted().await;
    let receipt = backend
        .copy_paths(
            id,
            moves(&[("stems/kick.wav", "mix.wav")]),
            OnConflict::KeepExisting,
        )
        .await
        .expect("keep existing");

    assert!(receipt.outcomes[0].skipped);
    assert_eq!(receipt.outcomes[0].landed_at, None);
    assert_eq!(
        std::fs::read(dir.join("mix.wav")).unwrap(),
        b"take one",
        "the occupant is untouched"
    );
}

// ── IntoSelf ────────────────────────────────────────────────────────

/// One guard, both call sites — a move or a copy into its own subtree is
/// the same mistake and gets the same typed fault.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn a_destination_inside_its_own_source_is_refused_for_move_and_copy() {
    let (_tmp, backend, id, dir) = adopted().await;
    let before = tree(&dir);

    for on_conflict in [OnConflict::Fail, OnConflict::Replace] {
        let err = backend
            .move_paths(id, moves(&[("stems", "stems/inner")]), on_conflict)
            .await
            .expect_err("into itself");
        assert!(matches!(err, FilesFault::IntoSelf { .. }), "move: {err:?}");

        let err = backend
            .copy_paths(id, moves(&[("stems", "stems/inner")]), on_conflict)
            .await
            .expect_err("into itself");
        assert!(matches!(err, FilesFault::IntoSelf { .. }), "copy: {err:?}");
    }
    assert_eq!(tree(&dir), before);
}

// ── Deletion ────────────────────────────────────────────────────────

/// Deleting removes it from the tree outright — no trash folder appears,
/// because "recently deleted" is a lens over history rather than a place.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn deletion_is_a_checkpoint_rather_than_a_move_to_trash() {
    let (_tmp, backend, id, dir) = adopted().await;

    let receipt = backend
        .delete_paths(id, vec![p("stems")])
        .await
        .expect("delete a whole subtree");

    assert!(!dir.join("stems").exists());
    assert_eq!(
        receipt.outcomes[0].landed_at, None,
        "a delete lands nowhere"
    );
    assert!(!receipt.operation.is_empty(), "and is one checkpoint");
    assert_eq!(
        tree(&dir),
        vec!["mix.wav".to_string()],
        "nothing is stored a second time anywhere in the tree"
    );
    assert!(
        !dir.join(files::STORE_DIR).join("write-staging").exists(),
        "and staging is gone once the checkpoint holds the content"
    );
}

// ── Confinement and validation ──────────────────────────────────────

/// `RootPath` is transparent on the wire, so a hostile peer's `..`
/// arrives having never seen `parse`. Every entry point re-validates.
#[tokio::test(flavor = "multi_thread")]
async fn a_wire_path_that_escapes_the_root_is_refused() {
    let (_tmp, backend, id, _dir) = adopted().await;
    let hostile: RootPath = serde_json::from_str("\"../escape\"").expect("transparent newtype");

    assert!(matches!(
        backend
            .create_dirs(id, vec![hostile.clone()])
            .await
            .expect_err("escape"),
        FilesFault::BadPath(_)
    ));
    assert!(matches!(
        backend
            .delete_paths(id, vec![hostile.clone()])
            .await
            .expect_err("escape"),
        FilesFault::BadPath(_)
    ));
    assert!(matches!(
        backend
            .move_paths(
                id,
                vec![Relocation {
                    from: p("mix.wav"),
                    to: hostile,
                }],
                OnConflict::Fail,
            )
            .await
            .expect_err("escape"),
        FilesFault::BadPath(_)
    ));
}

/// A rename takes a name, not a path: letting a separator through would
/// make it a silent move.
#[tokio::test(flavor = "multi_thread")]
async fn rename_refuses_a_name_that_is_really_a_path() {
    let (_tmp, backend, id, dir) = adopted().await;
    assert!(
        <FilesBackend as WriteService>::rename(&backend, id, p("mix.wav"), "../outside.wav".into())
            .await
            .is_err()
    );
    assert!(dir.join("mix.wav").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_root_is_a_typed_fault() {
    let (_tmp, backend, _id, _dir) = adopted().await;
    let ghost = RootId::generate();
    match backend
        .create_dirs(ghost, vec![p("x")])
        .await
        .expect_err("no such root")
    {
        FilesFault::RootNotFound(got) => assert_eq!(got, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }
}

/// A move whose destination folder does not exist is refused rather than
/// conjuring the folder — `create_dirs` is there for a caller who meant
/// it, and a typo must not manufacture a tree.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_into_a_missing_folder_is_refused() {
    let (_tmp, backend, id, dir) = adopted().await;
    let err = backend
        .move_paths(
            id,
            moves(&[("mix.wav", "Nowhere/mix.wav")]),
            OnConflict::Fail,
        )
        .await
        .expect_err("no such parent");
    assert!(matches!(err, FilesFault::PathNotFound(_)));
    assert!(dir.join("mix.wav").exists());
}

// ── Honestly unimplemented ──────────────────────────────────────────

/// An archive ticket promises only what a generated stream can keep.
///
/// This test used to pin the opposite: with no byte lane, minting a
/// ticket would have handed every caller a token that failed somewhere
/// else, so `archive` refused. The lane exists now, and the ticket says
/// plainly what it is — a tar produced in one pass, with no length known
/// in advance and no seeking.
// t[verify files.write.surface]
#[tokio::test(flavor = "multi_thread")]
async fn an_archive_ticket_promises_only_what_it_can_keep() {
    let (_tmp, backend, id, _dir) = adopted().await;
    let ticket = backend.archive(id, vec![p("stems")]).await.expect("ticket");
    assert_eq!(ticket.content_type, "application/x-tar");
    assert_eq!(
        ticket.length, None,
        "the size is not known until the archive has been produced"
    );
    assert!(!ticket.seekable, "one pass cannot seek");

    // An empty selection is a caller error, not an empty archive.
    assert!(matches!(
        backend.archive(id, vec![]).await,
        Err(FilesFault::Invalid(_))
    ));
}
