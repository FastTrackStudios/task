//! `VersionService` — the version/concurrency lane, against a real
//! `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, for the same reason
//! `roots_lane` is: the lane has no `permits.rs` rows yet, so it is not
//! mounted on a router and cannot be reached over vox. These call the
//! trait directly, which is what the dispatcher would do.
//!
//! Both traits are implemented on `FilesBackend` and four method names
//! collide (`chain`, `snapshots`, `divergences`, `resolve_divergence`), so
//! every call here is fully qualified — that ambiguity is the migration
//! showing through, and it disappears with the legacy impl.

use files::lane::version::version_id_of;
use files::FilesBackend;
use files_proto::id::{RootId, SnapshotId, VersionId};
use files_proto::model::RootFlavor;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::version::{Resolution, VersionService};
use files_proto::{FilesFault, RootPath};

/// A backend confined to its own data dir, with one adopted root holding
/// a project file and a stem.
async fn adopted(name: &str) -> (tempfile::TempDir, FilesBackend, RootId, std::path::PathBuf) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join(name);
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.rpp"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: name.to_string(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");
    let id = RootId::new(root.id);
    (data_dir, backend, id, dir)
}

fn path(p: &str) -> RootPath {
    RootPath::parse(p).expect("valid path")
}

// t[verify files.version.cadence]
#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_checkpoint_certifies_the_live_tree() {
    let (_tmp, backend, id, _dir) = adopted("session").await;

    let info = backend
        .checkpoint(id, Some("first pass".into()))
        .await
        .expect("checkpoint");
    assert_eq!(info.root_id, id.get());
    assert_eq!(info.description, "first pass");
    assert!(
        info.changed_paths.iter().any(|p| p == "mix.rpp"),
        "the certifying scan records what it committed: {:?}",
        info.changed_paths
    );
}

// t[verify files.version.unit]
#[tokio::test(flavor = "multi_thread")]
async fn a_chain_grows_one_entry_per_checkpoint() {
    let (_tmp, backend, id, dir) = adopted("chained").await;
    let file = path("mix.rpp");

    assert!(
        VersionService::chain(&backend, id, file.clone())
            .await
            .expect("chain")
            .is_empty(),
        "a path that has never been checkpointed has no chain"
    );

    backend.checkpoint(id, Some("v1".into())).await.unwrap();
    std::fs::write(dir.join("mix.rpp"), b"take two").unwrap();
    backend.checkpoint(id, Some("v2".into())).await.unwrap();

    let chain = VersionService::chain(&backend, id, file).await.expect("chain");
    assert_eq!(chain.len(), 2, "newest first: {chain:?}");
    assert_ne!(
        chain[0].file_id, chain[1].file_id,
        "two distinct saved states, addressed by content"
    );
}

// t[verify files.version.restore]
#[tokio::test(flavor = "multi_thread")]
async fn restoring_produces_a_new_version_and_discards_nothing() {
    let (_tmp, backend, id, dir) = adopted("restorable").await;
    let file = path("mix.rpp");

    backend.checkpoint(id, Some("v1".into())).await.unwrap();
    std::fs::write(dir.join("mix.rpp"), b"take two").unwrap();
    backend.checkpoint(id, Some("v2".into())).await.unwrap();

    let chain = VersionService::chain(&backend, id, file.clone()).await.unwrap();
    let older = version_id_of(&chain[1].commit_id);

    let restored = backend.restore(id, file.clone(), older).await.expect("restore");
    assert_eq!(
        std::fs::read(dir.join("mix.rpp")).unwrap(),
        b"take one",
        "the live tree holds the restored bytes"
    );

    let after = VersionService::chain(&backend, id, file).await.unwrap();
    assert_eq!(
        after.len(),
        3,
        "restoring adds a version rather than rewinding to one: {after:?}"
    );
    assert_eq!(after[0].commit_id, restored.commit_id);
    assert_eq!(
        after[0].file_id, chain[1].file_id,
        "and the new version's content is the old version's content"
    );
    assert_eq!(
        after[1].file_id, chain[0].file_id,
        "while the version restored *from* is still in history"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn restoring_an_unknown_version_is_a_typed_fault() {
    let (_tmp, backend, id, _dir) = adopted("unknown").await;
    backend.checkpoint(id, None).await.unwrap();

    let ghost = VersionId::generate();
    match backend
        .restore(id, path("mix.rpp"), ghost)
        .await
        .expect_err("no such version")
    {
        FilesFault::VersionNotFound(v) => assert_eq!(v, ghost),
        other => panic!("expected VersionNotFound, got {other:?}"),
    }
}

/// A `VersionId` is derived from the commit hex rather than stored, so the
/// same saved state must resolve to the same id every time it is read.
#[tokio::test(flavor = "multi_thread")]
async fn version_ids_are_stable_across_reads() {
    let (_tmp, backend, id, _dir) = adopted("stable").await;
    backend.checkpoint(id, None).await.unwrap();

    let a = VersionService::chain(&backend, id, path("mix.rpp")).await.unwrap();
    let b = VersionService::chain(&backend, id, path("mix.rpp")).await.unwrap();
    assert_eq!(
        version_id_of(&a[0].commit_id),
        version_id_of(&b[0].commit_id)
    );
}

// t[verify files.version.cadence]
#[tokio::test(flavor = "multi_thread")]
async fn snapshots_are_listed_apart_from_versions() {
    let (_tmp, backend, id, _dir) = adopted("snapped").await;
    backend.checkpoint(id, None).await.unwrap();

    let snaps = VersionService::snapshots(&backend, id, None)
        .await
        .expect("snapshots");
    assert!(
        snaps.is_empty(),
        "an explicit checkpoint is a version, never an auto-snapshot: {snaps:?}"
    );
    // The limit is honoured even when there is nothing to limit — a
    // recovery UI asks for the newest few unconditionally.
    assert!(
        VersionService::snapshots(&backend, id, Some(5))
            .await
            .unwrap()
            .is_empty()
    );
}

// t[verify files.version.keep-both]
#[tokio::test(flavor = "multi_thread")]
async fn a_single_headed_root_has_nothing_to_settle() {
    let (_tmp, backend, id, _dir) = adopted("undiverged").await;
    backend.checkpoint(id, None).await.unwrap();

    assert!(
        VersionService::divergences(&backend, id)
            .await
            .expect("divergences")
            .is_empty(),
        "one head, no divergence"
    );

    // And a resolution naming a version no divergence carries is refused
    // rather than resolving something arbitrary.
    let err = VersionService::resolve_divergence(
        &backend,
        id,
        VersionId::generate(),
        Resolution::KeepMine,
    )
    .await
    .expect_err("nothing to resolve");
    assert!(matches!(err, FilesFault::VersionNotFound(_)));
}

// t[verify files.concurrency.advisory-lock]
#[tokio::test(flavor = "multi_thread")]
async fn a_hold_is_published_and_gates_nothing() {
    let (_tmp, backend, id, dir) = adopted("held").await;
    let file = path("mix.rpp");

    assert!(
        backend.occupancy(id, file.clone()).await.unwrap().is_empty(),
        "nobody has it open yet"
    );

    let held = backend.hold(id, file.clone()).await.expect("hold");
    assert_eq!(held.root_id, id);
    assert_eq!(held.path, file);
    assert!(held.expires_at > held.since, "it lapses on its own");

    let who = backend.occupancy(id, file.clone()).await.expect("occupancy");
    assert_eq!(who.len(), 1);
    assert_eq!(who[0].principal, held.principal);

    // The whole point: the signal is advisory. A write and a checkpoint
    // against the held path go through untouched.
    std::fs::write(dir.join("mix.rpp"), b"take two").unwrap();
    let info = backend
        .checkpoint(id, Some("written while held".into()))
        .await
        .expect("a hold rejects no write");
    assert!(info.changed_paths.iter().any(|p| p == "mix.rpp"));

    // Idempotent: heartbeating produces one holder, not a queue of them.
    backend.hold(id, file.clone()).await.unwrap();
    backend.hold(id, file.clone()).await.unwrap();
    assert_eq!(backend.occupancy(id, file).await.unwrap().len(), 1);
}

// t[verify files.concurrency.advisory-lock]
#[tokio::test(flavor = "multi_thread")]
async fn occupancy_is_per_path() {
    let (_tmp, backend, id, _dir) = adopted("scoped").await;
    backend.hold(id, path("mix.rpp")).await.unwrap();
    assert!(
        backend
            .occupancy(id, path("stems/kick.wav"))
            .await
            .unwrap()
            .is_empty(),
        "holding the project file says nothing about a stem"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_verb_rejects_an_unknown_root() {
    let (_tmp, backend, _id, _dir) = adopted("real").await;
    let ghost = RootId::generate();

    let faults = vec![
        VersionService::chain(&backend, ghost, path("mix.rpp"))
            .await
            .expect_err("chain"),
        backend.checkpoint(ghost, None).await.expect_err("checkpoint"),
        VersionService::snapshots(&backend, ghost, None)
            .await
            .expect_err("snapshots"),
        backend.hold(ghost, path("mix.rpp")).await.expect_err("hold"),
        backend
            .occupancy(ghost, path("mix.rpp"))
            .await
            .expect_err("occupancy"),
        VersionService::divergences(&backend, ghost)
            .await
            .expect_err("divergences"),
        backend
            .restore(ghost, path("mix.rpp"), VersionId::generate())
            .await
            .expect_err("restore"),
        backend
            .keep_snapshot(ghost, SnapshotId::generate())
            .await
            .expect_err("keep_snapshot"),
    ];
    for fault in faults {
        assert!(
            matches!(fault, FilesFault::RootNotFound(id) if id == ghost),
            "expected RootNotFound, got {fault:?}"
        );
    }
}

/// `keep_snapshot` is deliberately unimplemented — promoting a snapshot
/// moves the checkpoint head onto a branch that exists to stay off it.
/// Pinned as a test so it is a decision on record rather than an oversight
/// someone rediscovers at runtime.
#[tokio::test(flavor = "multi_thread")]
async fn keeping_a_snapshot_is_not_yet_implemented() {
    let (_tmp, backend, id, _dir) = adopted("promote").await;
    match backend
        .keep_snapshot(id, SnapshotId::generate())
        .await
        .expect_err("not implemented")
    {
        FilesFault::Internal(m) => assert!(m.starts_with("not yet implemented:"), "{m}"),
        other => panic!("expected Internal, got {other:?}"),
    }
}
