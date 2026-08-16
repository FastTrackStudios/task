//! `UploadService` — the upload lane, against a real `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, for the reason
//! `roots_lane.rs` gives: the v2 lanes have no `permits.rs` rows yet, so
//! nothing is mounted on a router and the gate fails closed. These call
//! the trait directly, which is what the dispatcher would do.
//!
//! What is exercised here is everything `files.write.upload` asks for
//! that does not need a byte lane — there is no byte lane, and the lane
//! says so rather than inventing one:
//!
//! - **Dedup before transfer.** Content the destination root's chunk
//!   store already holds plans an empty `needed` list and completes
//!   without a byte moving.
//! - **Resumption.** `progress` is re-derived from the store on every
//!   call, so chunks that arrive while a session is open are counted
//!   against it and are never re-sent.
//! - **The collision asks.** All four `OnConflict` arms, and the plan
//!   that reports the occupant *and* the keep-both name before anyone
//!   chooses.
//! - **Nothing partial is visible.** An aborted upload leaves the tree
//!   exactly as it found it.
//!
//! Session state is process-global (see the lane's module doc), and
//! integration tests share a process — so anything asserting over
//! `pending()` checks for its own ids rather than for a count.

use files::FilesBackend;
use files_proto::id::{ContentId, RootId, UploadId};
use files_proto::model::RootFlavor;
use files_proto::service::legacy::FilesService as LegacyFiles;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::upload::{UploadService, UploadSpec};
use files_proto::service::write::OnConflict;
use files_proto::{FilesFault, RootPath};

struct Rig {
    _tmp: tempfile::TempDir,
    data: std::path::PathBuf,
    dir: std::path::PathBuf,
    backend: FilesBackend,
    root: RootId,
}

/// A media root holding `mix.wav` and `stems/kick.wav`, checkpointed so
/// both files' content is in its chunk store and addressable.
async fn rig() -> Rig {
    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Mix Session".into(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");
    let rig = Rig {
        data: tmp.path().to_path_buf(),
        _tmp: tmp,
        dir,
        backend,
        root: RootId::new(root.id),
    };
    rig.checkpoint("staged").await;
    rig
}

impl Rig {
    async fn checkpoint(&self, why: &str) {
        LegacyFiles::checkpoint_now(&self.backend, self.root.get(), Some(why.into()))
            .await
            .expect("checkpoint");
    }

    /// A file's content address as a client would compute one, plus its
    /// length — the two halves of a dedup-capable [`UploadSpec`].
    async fn address(&self, path: &str) -> (ContentId, u64) {
        let (len, hex) = self
            .backend
            .resolve_source(self.root.get(), path.into(), None)
            .await
            .expect("resolve source");
        (ContentId::new(hex), len)
    }

    /// A second media root under the same data dir, with its own chunk
    /// store — so "the store already holds this" can be made false.
    async fn other_root(&self, name: &str) -> (RootId, std::path::PathBuf) {
        let dir = self.data.join(name);
        std::fs::create_dir(&dir).unwrap();
        let root = self
            .backend
            .adopt(AdoptRequest {
                path: dir.to_string_lossy().into_owned(),
                name: name.into(),
                flavor: RootFlavor::Media,
                hash_content: true,
            })
            .await
            .expect("adopt");
        (RootId::new(root.id), dir)
    }

    fn spec(&self, path: &str, content: Option<ContentId>, size: u64) -> UploadSpec {
        UploadSpec {
            root_id: self.root,
            path: RootPath::parse(path).expect("path"),
            size,
            content,
            modified_at: None,
        }
    }

    async fn chain_len(&self, path: &str) -> usize {
        LegacyFiles::chain(&self.backend, self.root.get(), path.into())
            .await
            .expect("chain")
            .len()
    }
}

// ── Dedup ──────────────────────────────────────────────────────────

/// The acceptance criterion the content-addressed store exists for: an
/// upload of content the server already holds asks for no bytes at all,
/// and the answer is reached before any transfer rather than after one.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn content_the_server_already_holds_transfers_nothing() {
    let rig = rig().await;
    let (content, size) = rig.address("mix.wav").await;

    let plan = rig
        .backend
        .begin(rig.spec("Audio Files/vox.wav", Some(content), size))
        .await
        .expect("begin");

    assert!(
        plan.needed.is_empty(),
        "the store holds this content; an empty needed list is the whole answer: {:?}",
        plan.needed
    );
    assert!(plan.conflict.is_none(), "the destination is free");
    assert_eq!(
        plan.lane,
        files::lane::upload::BYTE_LANE,
        "the plan names the placeholder lane rather than an endpoint that does not exist"
    );

    let progress = rig.backend.progress(plan.upload_id).await.expect("progress");
    assert_eq!(progress.received, size);
    assert_eq!(progress.total, size);
    assert!(progress.needed.is_empty());

    // And it completes for real: the bytes come out of the CAS, so the
    // dedup path is a landing rather than a promise.
    let entry = rig
        .backend
        .complete(plan.upload_id, OnConflict::Fail)
        .await
        .expect("complete");
    assert_eq!(entry.path.as_str(), "Audio Files/vox.wav");
    assert_eq!(entry.size, size);
    assert_eq!(
        std::fs::read(rig.dir.join("Audio Files/vox.wav")).unwrap(),
        b"take one",
        "the landed bytes are the ones that address resolves to"
    );
}

/// Without a client content address there is nothing to compare, so the
/// whole file is asked for. A server cannot derive the address without
/// the bytes it is trying not to ask for, and the lane reports that
/// rather than pretending otherwise.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_with_no_content_address_pays_in_full() {
    let rig = rig().await;
    let plan = rig
        .backend
        .begin(rig.spec("Audio Files/unknown.wav", None, 4_096))
        .await
        .expect("begin");

    assert_eq!(plan.needed.len(), 1);
    assert_eq!(plan.needed[0].start, 0);
    assert_eq!(plan.needed[0].end, 4_096);

    // And it cannot be completed, because nothing in this codebase
    // receives bytes yet. A decision on record, not a silent truncation.
    match rig
        .backend
        .complete(plan.upload_id, OnConflict::Fail)
        .await
        .expect_err("no byte lane exists")
    {
        FilesFault::Internal(why) => assert!(
            why.contains("not yet implemented: the byte lane"),
            "the refusal must name what is missing: {why}"
        ),
        other => panic!("expected the byte-lane refusal, got {other:?}"),
    }

    // The session survives the refusal, so nothing is lost when the lane
    // does land.
    assert!(rig.backend.progress(plan.upload_id).await.is_ok());
    assert!(
        !rig.dir.join("Audio Files/unknown.wav").exists(),
        "and no partial file is visible in the live tree"
    );
}

/// A content address whose manifest describes a different number of
/// bytes than the spec declares is a lie in one of the two fields, and
/// which one cannot be known. It is refused rather than reconciled.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn a_size_that_contradicts_the_content_address_is_refused() {
    let rig = rig().await;
    let (content, size) = rig.address("mix.wav").await;
    let err = rig
        .backend
        .begin(rig.spec("Audio Files/liar.wav", Some(content), size + 1))
        .await
        .expect_err("the address and the size disagree");
    assert!(matches!(err, FilesFault::Invalid(_)), "got {err:?}");
}

// ── Resumption ─────────────────────────────────────────────────────

/// Resumption is a fact read out of the chunk store, not a counter held
/// in a session: chunks that arrive while an upload is open are counted
/// against it, and the client is never asked to re-send them.
///
/// The chunk import here is what the byte lane would do when it exists —
/// the same store API `files-sync` uses to land a transferred chunk.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_upload_resumes_rather_than_restarting() {
    let rig = rig().await;
    let (content, size) = rig.address("mix.wav").await;
    let (other, other_dir) = rig.other_root("incoming").await;

    // A root that has never seen this content needs all of it.
    let plan = rig
        .backend
        .begin(UploadSpec {
            root_id: other,
            path: RootPath::parse("takes/mix.wav").unwrap(),
            size,
            content: Some(content.clone()),
            modified_at: None,
        })
        .await
        .expect("begin");
    assert_eq!(
        plan.needed,
        vec![files_proto::service::upload::ChunkRange {
            start: 0,
            end: size
        }],
        "a store that holds none of it asks for all of it"
    );
    assert_eq!(
        rig.backend
            .progress(plan.upload_id)
            .await
            .expect("progress")
            .received,
        0
    );

    // The transfer happens (here: by hand, chunk by chunk, exactly as a
    // byte lane would land them) and then the connection is notionally
    // dropped — nothing tells the session anything.
    let manifest = rig
        .backend
        .sync_manifest(rig.root.get(), content.as_str())
        .expect("manifest");
    assert!(!manifest.is_empty(), "content has at least one chunk");
    for (hash, _len) in &manifest {
        let bytes = rig
            .backend
            .sync_read_chunk(rig.root.get(), hash)
            .expect("read chunk");
        rig.backend
            .sync_import_chunk(other.get(), hash, bytes)
            .expect("import chunk");
    }
    rig.backend
        .sync_import_manifest(other.get(), content.as_str(), manifest)
        .expect("import manifest");

    // The same session, asked again, reports the truth rather than what
    // it last remembered.
    let resumed = rig.backend.progress(plan.upload_id).await.expect("progress");
    assert!(
        resumed.needed.is_empty(),
        "what arrived is not asked for again: {:?}",
        resumed.needed
    );
    assert_eq!(resumed.received, size);
    assert_eq!(resumed.upload_id, plan.upload_id, "and it is the same upload");

    // Which means it can now land — no restart, no re-transfer.
    rig.backend
        .complete(plan.upload_id, OnConflict::Fail)
        .await
        .expect("complete");
    assert_eq!(
        std::fs::read(other_dir.join("takes/mix.wav")).unwrap(),
        b"take one"
    );
}

// ── The collision asks ─────────────────────────────────────────────

/// The plan reports the occupant and the name keep-both would use
/// *before* anyone chooses, so a human decides knowing what they would
/// displace and what they would end up with.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn a_collision_is_reported_before_the_choice_is_made() {
    let rig = rig().await;
    let (content, size) = rig.address("mix.wav").await;

    let plan = rig
        .backend
        .begin(rig.spec("mix.wav", Some(content), size))
        .await
        .expect("begin");

    let conflict = plan.conflict.expect("the destination is occupied");
    assert_eq!(conflict.existing.path.as_str(), "mix.wav");
    assert_eq!(conflict.existing.size, size);
    assert_eq!(
        conflict.keep_both_as.as_str(),
        "mix (2).wav",
        "the keep-both name is shown before choosing, not after"
    );
    assert!(
        rig.dir.join("mix.wav").exists() && !rig.dir.join("mix (2).wav").exists(),
        "and reporting a collision changes nothing on disk"
    );
}

/// `Fail` refuses and keeps the session: the question has been bounced
/// back to a human, and making them re-upload to answer it would be the
/// coercion this lane exists to prevent.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn fail_refuses_and_leaves_the_upload_open() {
    let rig = rig().await;
    let (content, size) = rig.address("mix.wav").await;
    let plan = rig
        .backend
        .begin(rig.spec("mix.wav", Some(content), size))
        .await
        .expect("begin");

    match rig
        .backend
        .complete(plan.upload_id, OnConflict::Fail)
        .await
        .expect_err("the destination is occupied")
    {
        FilesFault::Exists { path } => assert_eq!(path.as_str(), "mix.wav"),
        other => panic!("expected Exists, got {other:?}"),
    }

    assert!(
        rig.backend.progress(plan.upload_id).await.is_ok(),
        "the upload is still there to be resolved another way"
    );
    assert_eq!(std::fs::read(rig.dir.join("mix.wav")).unwrap(), b"take one");
}

// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn keep_both_lands_beside_the_occupant() {
    let rig = rig().await;
    let (content, size) = rig.address("stems/kick.wav").await;
    let plan = rig
        .backend
        .begin(rig.spec("mix.wav", Some(content), size))
        .await
        .expect("begin");

    let entry = rig
        .backend
        .complete(plan.upload_id, OnConflict::KeepBoth)
        .await
        .expect("complete");

    assert_eq!(entry.path.as_str(), "mix (2).wav");
    assert_eq!(
        std::fs::read(rig.dir.join("mix.wav")).unwrap(),
        b"take one",
        "the occupant is untouched"
    );
    assert_eq!(
        std::fs::read(rig.dir.join("mix (2).wav")).unwrap(),
        b"boom",
        "and the incoming content lands beside it"
    );
}

// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn keep_existing_drops_the_incoming_and_touches_nothing() {
    let rig = rig().await;
    let (content, size) = rig.address("stems/kick.wav").await;
    let plan = rig
        .backend
        .begin(rig.spec("mix.wav", Some(content), size))
        .await
        .expect("begin");

    let entry = rig
        .backend
        .complete(plan.upload_id, OnConflict::KeepExisting)
        .await
        .expect("complete");

    assert_eq!(
        entry.path.as_str(),
        "mix.wav",
        "the answer is the existing entry, not a new one"
    );
    assert_eq!(std::fs::read(rig.dir.join("mix.wav")).unwrap(), b"take one");
    assert!(!rig.dir.join("mix (2).wav").exists());
    assert!(
        matches!(
            rig.backend.progress(plan.upload_id).await,
            Err(FilesFault::UploadNotFound(_))
        ),
        "the upload is finished, whichever way it went"
    );
}

/// The rule that separates a version store from a filesystem: replacing
/// records the outgoing content as a version instead of destroying it.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn replace_records_a_new_version_rather_than_discarding_the_old() {
    let rig = rig().await;
    let (content, size) = rig.address("stems/kick.wav").await;
    let before = rig.chain_len("mix.wav").await;

    let plan = rig
        .backend
        .begin(rig.spec("mix.wav", Some(content), size))
        .await
        .expect("begin");
    let entry = rig
        .backend
        .complete(plan.upload_id, OnConflict::Replace)
        .await
        .expect("complete");

    assert_eq!(entry.path.as_str(), "mix.wav");
    assert_eq!(
        std::fs::read(rig.dir.join("mix.wav")).unwrap(),
        b"boom",
        "the live tree holds the incoming content"
    );
    assert!(
        rig.chain_len("mix.wav").await > before,
        "and the displaced content is still in the chain, recoverable"
    );
}

// ── Abort, and what is never visible ───────────────────────────────

/// Nothing partial is ever visible in the live tree, so aborting is the
/// absence of an action rather than the reversal of one.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_upload_leaves_nothing_behind() {
    let rig = rig().await;
    let (content, size) = rig.address("mix.wav").await;
    let plan = rig
        .backend
        .begin(rig.spec("Audio Files/aborted.wav", Some(content), size))
        .await
        .expect("begin");

    assert!(
        !rig.dir.join("Audio Files").exists(),
        "beginning an upload creates nothing in the tree"
    );

    rig.backend.abort(plan.upload_id).await.expect("abort");

    assert!(
        !rig.dir.join("Audio Files/aborted.wav").exists(),
        "and aborting leaves nothing behind"
    );
    assert!(matches!(
        rig.backend.progress(plan.upload_id).await,
        Err(FilesFault::UploadNotFound(_))
    ));
    // Aborting twice is not silently fine: the second call is about an
    // upload that no longer exists, and saying so is how a client learns
    // its view is stale.
    assert!(matches!(
        rig.backend.abort(plan.upload_id).await,
        Err(FilesFault::UploadNotFound(_))
    ));
}

// ── Typed faults, and pending ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_upload_is_a_typed_fault() {
    let rig = rig().await;
    let ghost = UploadId::generate();
    match rig.backend.progress(ghost).await.expect_err("no such upload") {
        FilesFault::UploadNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected UploadNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_into_an_unknown_root_is_refused_before_anything_else() {
    let rig = rig().await;
    let ghost = RootId::generate();
    let spec = UploadSpec {
        root_id: ghost,
        ..rig.spec("mix.wav", None, 1)
    };
    match rig.backend.begin(spec).await.expect_err("no such root") {
        FilesFault::RootNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_root_itself_is_not_a_destination() {
    let rig = rig().await;
    let err = rig
        .backend
        .begin(rig.spec("", None, 1))
        .await
        .expect_err("the root is not a file");
    assert!(matches!(err, FilesFault::Invalid(_)), "got {err:?}");
}

/// Open uploads are listable so a client that lost its session ids can
/// find them again. Sessions are process-global and the test binary is
/// one process, so this asserts over its own ids rather than a count.
// t[verify files.write.upload]
#[tokio::test(flavor = "multi_thread")]
async fn pending_lists_what_is_still_open() {
    let rig = rig().await;
    let (content, size) = rig.address("mix.wav").await;
    let one = rig
        .backend
        .begin(rig.spec("Audio Files/a.wav", Some(content.clone()), size))
        .await
        .expect("begin");
    let two = rig
        .backend
        .begin(rig.spec("Audio Files/b.wav", Some(content), size))
        .await
        .expect("begin");

    let pending = rig.backend.pending().await.expect("pending");
    assert!(pending.iter().any(|p| p.upload_id == one.upload_id));
    assert!(pending.iter().any(|p| p.upload_id == two.upload_id));

    rig.backend.abort(one.upload_id).await.expect("abort");
    let pending = rig.backend.pending().await.expect("pending");
    assert!(
        !pending.iter().any(|p| p.upload_id == one.upload_id),
        "an aborted upload is no longer pending"
    );
    assert!(pending.iter().any(|p| p.upload_id == two.upload_id));
}
