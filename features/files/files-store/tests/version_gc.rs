//! Issue #258 acceptance criteria: `gc(index, keep_newer)` mark-and-sweep
//! honoring a protect callback (Vault-referenced versions, supplied here as
//! `protected_commits`) and the retention contract (`keep_newer` guards
//! concurrent writers), plus chunk-level reclamation layered on top of
//! #257's structural-object sweep. Exercised directly against
//! `crate::gc::sweep` (the spec's Testing Decisions "secondary harness"),
//! matching `tests/acceptance.rs`'s own posture.
//!
//! `sweep`'s chunk-store phase durably removes swept manifests, but the
//! underlying chunk *blobs* are reclaimed asynchronously on iroh-blobs' own
//! background schedule (see `task-files-chunk-store`'s `gc` module doc) —
//! tests that need to observe that poll with a bounded timeout against a
//! short GC interval (`init_repo_with_gc_interval`), rather than asserting
//! immediately after `sweep` returns.

use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use jj_lib::backend::{
    Backend as _, ChangeId, Commit, MillisSinceEpoch, Signature, Timestamp, TreeValue,
};
use jj_lib::merge::Merge;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::tree_builder::TreeBuilder;
use tokio_util::compat::TokioAsyncReadCompatExt as _;

use files_store::version::VersionStoreBackend;
use files_store::version::gc::sweep;
use files_store::version::repo::init_repo_with_gc_interval;

const GC_INTERVAL: Duration = Duration::from_millis(30);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const POLL_TIMEOUT: Duration = Duration::from_secs(5);

fn path(s: &str) -> RepoPathBuf {
    RepoPathBuf::from_internal_string(s).unwrap()
}

fn backend_of(repo: &Arc<ReadonlyRepo>) -> &VersionStoreBackend {
    repo.store()
        .backend_impl::<VersionStoreBackend>()
        .expect("repo's store is a VersionStoreBackend")
}

async fn init(dir: &std::path::Path) -> Arc<ReadonlyRepo> {
    init_repo_with_gc_interval(&dir.join("repo"), GC_INTERVAL)
        .await
        .unwrap()
}

fn chunk_id_of(jj_id: &jj_lib::backend::FileId) -> files_store::chunk::FileId {
    files_store::chunk::FileId::from_hex(&jj_id.hex()).unwrap()
}

/// Poll until `id` is gone from the chunk store, or time out — the chunk
/// blob itself is reclaimed on iroh-blobs' own background schedule (see the
/// module doc), not synchronously within `sweep`.
async fn wait_until_chunk_absent(
    backend: &VersionStoreBackend,
    id: files_store::chunk::FileId,
) {
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    loop {
        if !backend.chunks().has(id).await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "chunk was never reclaimed by the background sweep"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn dummy_signature() -> Signature {
    Signature {
        name: String::new(),
        email: String::new(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        },
    }
}

/// Writes `content` at `path` and wraps it in a commit on top of the root
/// commit, but writes that commit *straight through the `Backend` trait*
/// (`write_tree`/`write_commit`) rather than through a jj transaction — so
/// unlike a real checkpoint, it never becomes a view head and is not
/// reachable from `index.all_heads_for_gc()`. This is what lets the
/// protect-callback test below exercise "reachable only because the Vault
/// says so" — the same shape a Named Version pointing at an old,
/// no-longer-current commit has per ADR 0001.
async fn write_orphan_commit(
    repo: &Arc<ReadonlyRepo>,
    path: &jj_lib::repo_path::RepoPath,
    content: &[u8],
) -> (jj_lib::backend::CommitId, jj_lib::backend::FileId) {
    let backend = backend_of(repo);
    let file_id = backend
        .write_file(path, &mut Cursor::new(content.to_vec()).compat())
        .await
        .unwrap();
    let copy_id = backend.write_origin_copy(path, vec![]).await.unwrap();

    let store = repo.store().clone();
    let empty_tree_id = jj_lib::backend::Backend::empty_tree_id(backend).clone();
    let mut builder = TreeBuilder::new(store, empty_tree_id);
    builder.set(
        path.to_owned(),
        TreeValue::File {
            id: file_id.clone(),
            executable: false,
            copy_id,
        },
    );
    let tree_id = builder.write_tree().await.unwrap();

    let commit = Commit {
        parents: vec![repo.store().root_commit_id().clone()],
        predecessors: vec![],
        root_tree: Merge::from_vec(vec![tree_id]),
        conflict_labels: Merge::from_vec(vec![String::new()]),
        change_id: ChangeId::new(vec![0xAB; 16]),
        description: "orphan (vault-referenced only)".into(),
        author: dummy_signature(),
        committer: dummy_signature(),
        secure_sig: None,
    };
    let (commit_id, _) = jj_lib::backend::Backend::write_commit(backend, commit, None)
        .await
        .unwrap();
    (commit_id, file_id)
}

/// "Unreferenced expired snapshots are swept; their unshared chunks are
/// reclaimed."
#[tokio::test]
async fn unreferenced_expired_file_is_swept_and_its_chunk_reclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init(dir.path()).await;
    let backend = backend_of(&repo);

    // Written directly through the `Backend` trait, never placed in any
    // tree/commit — an orphan, exactly like an ephemeral auto-snapshot that
    // nothing (no live checkpoint, no Vault reference) ever points at.
    let file_id = backend
        .write_file(
            &path("orphan.wav"),
            &mut Cursor::new(b"nobody keeps this".to_vec()).compat(),
        )
        .await
        .unwrap();
    let chunk_id = chunk_id_of(&file_id);
    assert!(backend.chunks().has(chunk_id).await);

    tokio::time::sleep(Duration::from_millis(10)).await;
    let keep_newer = SystemTime::now();

    let stats = sweep(backend, repo.readonly_index().as_index(), keep_newer, &[])
        .await
        .unwrap();
    assert_eq!(stats.chunks.manifests_swept, 1);

    // The manifest is gone immediately (the mark phase is synchronous).
    assert!(!backend.chunks().has(chunk_id).await);

    // The chunk blob itself is reclaimed on iroh-blobs' own schedule.
    wait_until_chunk_absent(backend, chunk_id).await;
}

/// "Protect-callback-listed versions survive GC regardless of age."
#[tokio::test]
async fn protected_commit_survives_gc_regardless_of_age() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init(dir.path()).await;
    let backend = backend_of(&repo);

    let (protected_commit, protected_file) =
        write_orphan_commit(&repo, &path("v3-for-client.wav"), b"a Named Version").await;
    let protected_chunk = chunk_id_of(&protected_file);

    // An equally orphaned, equally old commit that the Vault does *not*
    // reference — the control: without protection, this shape is exactly
    // what gets swept (see the sibling test above).
    let (_unprotected_commit, unprotected_file) =
        write_orphan_commit(&repo, &path("scratch.wav"), b"nobody points at this one").await;
    let unprotected_chunk = chunk_id_of(&unprotected_file);

    tokio::time::sleep(Duration::from_millis(10)).await;
    let keep_newer = SystemTime::now();

    let stats = sweep(
        backend,
        repo.readonly_index().as_index(),
        keep_newer,
        std::slice::from_ref(&protected_commit),
    )
    .await
    .unwrap();
    assert_eq!(
        stats.chunks.manifests_swept, 1,
        "only the unprotected orphan's manifest should be swept"
    );

    assert!(
        backend.chunks().has(protected_chunk).await,
        "a commit named by the protect set must survive regardless of age"
    );
    assert!(!backend.chunks().has(unprotected_chunk).await);
    wait_until_chunk_absent(backend, unprotected_chunk).await;
}

/// Regression test (data-loss finding): `Backend::gc` — jj-lib's own
/// generic trait-method entry point, reachable by any jj-lib-native caller,
/// not just this crate's Vault-aware one — used to call `crate::gc::sweep`
/// with a hardcoded empty `protected_commits`, so a commit that's real and
/// Vault-referenced but not currently `index`-reachable (exactly the shape
/// `write_orphan_commit` builds: a Named Version pointing at a commit that
/// isn't a current view head) had its manifest durably swept the moment
/// anything called the trait method — even though nothing in the tree
/// resolves Vault references yet to supply a real `protected_commits`.
/// `Backend::gc` now never touches the chunk store at all (see `gc.rs`'s
/// module doc), so this must survive it regardless.
#[tokio::test]
async fn backend_gc_trait_method_never_sweeps_the_chunk_store() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init(dir.path()).await;
    let backend = backend_of(&repo);

    let (_vault_only_commit, vault_only_file) = write_orphan_commit(
        &repo,
        &path("v3-for-client.wav"),
        b"a Named Version, index-unreachable, not passed to Backend::gc",
    )
    .await;
    let vault_only_chunk = chunk_id_of(&vault_only_file);

    tokio::time::sleep(Duration::from_millis(10)).await;

    // The trait method itself — `jj_lib::backend::Backend::gc`, not
    // `crate::gc::sweep` — is what a generic jj-lib caller (e.g. `jj util
    // gc`) would actually reach.
    backend
        .gc(repo.readonly_index().as_index(), SystemTime::now())
        .unwrap();

    assert!(
        backend.chunks().has(vault_only_chunk).await,
        "Backend::gc must never sweep the chunk store — a Vault-only-referenced \
         commit's manifest must survive it regardless of index reachability"
    );
    assert_eq!(
        backend
            .chunks()
            .read_to_vec(vault_only_chunk)
            .await
            .unwrap(),
        b"a Named Version, index-unreachable, not passed to Backend::gc"
    );
}

/// "keep_newer guards concurrent writers (nothing newer is swept)."
#[tokio::test]
async fn keep_newer_protects_a_file_written_after_the_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init(dir.path()).await;
    let backend = backend_of(&repo);

    let keep_newer = SystemTime::now();
    tokio::time::sleep(Duration::from_millis(10)).await;
    // Written *after* keep_newer, orphaned, and never protected: only its
    // mtime — the concurrent-writer guard — saves it from this pass.
    let file_id = backend
        .write_file(
            &path("mid-write.wav"),
            &mut Cursor::new(b"a save racing this gc pass".to_vec()).compat(),
        )
        .await
        .unwrap();
    let chunk_id = chunk_id_of(&file_id);

    let stats = sweep(backend, repo.readonly_index().as_index(), keep_newer, &[])
        .await
        .unwrap();
    assert_eq!(
        stats.chunks.manifests_swept, 0,
        "a manifest newer than keep_newer must never be swept"
    );
    assert!(backend.chunks().has(chunk_id).await);
}

/// "Store size shrinks measurably after sweeping a churn-heavy history."
#[tokio::test]
async fn store_shrinks_measurably_after_sweeping_a_churn_heavy_history() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init(dir.path()).await;
    let backend = backend_of(&repo);

    // Simulate a tracking day's worth of expired auto-snapshots: many
    // distinct-content orphan writes, none of them a live checkpoint or a
    // Vault reference.
    for i in 0..30u32 {
        let content = format!("take {i}: {}", "x".repeat(4096));
        backend
            .write_file(
                &path(&format!("session/take-{i}.wav")),
                &mut Cursor::new(content.into_bytes()).compat(),
            )
            .await
            .unwrap();
    }
    let before = backend.chunks().chunk_count().await.unwrap();
    assert!(
        before >= 30,
        "expected at least one chunk per churn write, got {before}"
    );

    tokio::time::sleep(Duration::from_millis(10)).await;
    let keep_newer = SystemTime::now();

    let stats = sweep(backend, repo.readonly_index().as_index(), keep_newer, &[])
        .await
        .unwrap();
    assert_eq!(stats.chunks.manifests_swept, 30);

    // Chunk reclamation is on iroh-blobs' own schedule (see module doc);
    // poll until the store measurably shrinks.
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    let after = loop {
        let count = backend.chunks().chunk_count().await.unwrap();
        if count == 0 || tokio::time::Instant::now() >= deadline {
            break count;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        after, 0,
        "expected the entire churn-heavy history's chunks to be reclaimed: before={before} after={after}"
    );
}
