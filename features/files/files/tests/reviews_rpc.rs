//! Reviews (issue #270, Phase B) at the RPC seam: the review entity,
//! timecoded comments recording their file version, and annotation
//! round-tripping — over an in-process `LocalServer`, against real
//! vault pages on disk.

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, RootFlavor};
use files_proto::service::media::Region;
use files_proto::service::review::NewComment;
use files_proto::service::roots::AdoptRequest;
use files_proto::{
    AnnotationPoint, AnnotationStroke, CommentId, FilesFault, ReviewId, ReviewServiceClient,
    RootId, RootPath, RootsServiceClient, VersionId, VersionServiceClient,
};

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files_proto::roots_layer(backend.clone()))
        .merge(files_proto::version_layer(backend.clone()))
        .merge(files_proto::review_layer(backend))
}

/// The lanes this file drives, over one in-process link.
struct Rig {
    backend: FilesBackend,
    version: VersionServiceClient,
    review: ReviewServiceClient,
    _local: LocalServer,
}

impl Rig {
    /// The file's version chain, newest first.
    async fn chain(&self, root_id: RootId) -> Vec<files::ChainEntry> {
        self.version.chain(root_id, cut()).await.unwrap()
    }
}

fn cut() -> RootPath {
    RootPath::parse("cut.mov").unwrap()
}

/// The domain fault inside a transport error, as text.
fn fault_text(err: vox::VoxError<FilesFault>) -> String {
    match err {
        vox::VoxError::User(fault) => fault.to_string(),
        other => panic!("expected a domain fault, got {other:?}"),
    }
}

/// A media root with one video file, checkpointed.
async fn rig() -> (tempfile::TempDir, Rig, RootId) {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).unwrap();
    let root_dir = dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("cut.mov"), vec![0x11u8; 2048]).unwrap();
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let roots: RootsServiceClient = local.establish().await.unwrap();
    let root = roots
        .adopt(AdoptRequest {
            path: root_dir.to_string_lossy().into_owned(),
            name: "session".into(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .unwrap();
    let root_id = RootId::new(root.id);
    backend.settled(root_id).await;
    let rig = Rig {
        backend,
        version: local.establish().await.unwrap(),
        review: local.establish().await.unwrap(),
        _local: local,
    };
    rig.version.checkpoint(root_id, None).await.unwrap();
    (dir, rig, root_id)
}

/// A timecoded comment in the v2 shape: the moment as a zero-length
/// time region, the version as the commit it was made on.
fn comment_at(review: ReviewId, ms: u64, body: &str, commit_id: &str) -> NewComment {
    NewComment {
        review,
        version: VersionId::from_commit_hex(commit_id),
        region: Region::Time {
            start_ms: ms,
            end_ms: ms,
        },
        body: body.into(),
        strokes: Vec::new(),
        author: "Cody".into(),
    }
}

/// One review per file, created on first ask; comments order by
/// timecode and record the exact (normalized) version they were made
/// on — and a new version of the file keeps them attributed (AC 2).
#[tokio::test(flavor = "multi_thread")]
async fn comments_pin_their_file_version_across_new_versions() {
    let (dir, rig, root_id) = rig().await;

    // Get-or-create: two asks, one entity.
    let review = rig
        .review
        .for_file(root_id, cut())
        .await
        .expect("create review");
    let again = rig
        .review
        .for_file(root_id, cut())
        .await
        .expect("same review");
    assert_eq!(review.id, again.id, "one review per (root, file)");
    assert_eq!(review.title, "cut.mov");
    assert_eq!(
        rig.review.reviews(Some(root_id)).await.unwrap().len(),
        1,
        "listed once"
    );
    let review_id = ReviewId::new(review.id);

    // Comment on version 1 — the chain's head commit.
    let v1 = &rig.chain(root_id).await[0].commit_id.clone();
    let c1 = rig
        .review
        .comment(comment_at(review_id, 12_500, "logo too early", v1))
        .await
        .expect("comment on v1");
    assert_eq!(&c1.commit_id, v1, "records the version it was made on");
    assert_eq!(c1.author, "Cody");

    // A new version of the file lands…
    std::fs::write(dir.path().join("session/cut.mov"), vec![0x22u8; 4096]).unwrap();
    rig.version.checkpoint(root_id, None).await.unwrap();
    let chain = rig.chain(root_id).await;
    assert!(chain.len() >= 2, "two versions now: {}", chain.len());
    let v2 = &chain[0].commit_id;
    assert_ne!(v1, v2);

    // …and a comment on v2 joins the SAME review, while the v1 comment
    // keeps its original attribution.
    let c2 = rig
        .review
        .comment(comment_at(review_id, 3_000, "new cut fixes it", v2))
        .await
        .expect("comment on v2");
    assert_eq!(&c2.commit_id, v2);
    let comments = rig.review.comments(review_id).await.unwrap();
    assert_eq!(comments.len(), 2);
    // Ordered by timecode, not creation: 3.0s before 12.5s.
    assert_eq!(comments[0].id, c2.id);
    assert_eq!(comments[1].id, c1.id);
    assert_eq!(
        &comments[1].commit_id, v1,
        "old comment attribution survives"
    );

    // Delete removes the page.
    rig.review
        .delete_comment(CommentId::new(c2.id))
        .await
        .unwrap();
    assert_eq!(rig.review.comments(review_id).await.unwrap().len(), 1);

    rig.backend.shutdown().await;
}

/// A frame drawing round-trips through the vault page in normalized
/// coordinates (AC 3's persistence half).
#[tokio::test(flavor = "multi_thread")]
async fn annotations_round_trip_through_the_vault_page() {
    let (_dir, rig, root_id) = rig().await;
    let review = rig.review.for_file(root_id, cut()).await.unwrap();
    let review_id = ReviewId::new(review.id);
    let head = rig.chain(root_id).await[0].commit_id.clone();

    let stroke = AnnotationStroke {
        points: vec![
            AnnotationPoint { x: 0.1, y: 0.2 },
            AnnotationPoint { x: 0.5, y: 0.75 },
        ],
        color: "#ff3355".into(),
        width: 0.004,
    };
    let drawn = rig
        .review
        .comment(NewComment {
            strokes: vec![stroke.clone()],
            author: String::new(),
            ..comment_at(review_id, 7_250, "circle this", &head)
        })
        .await
        .expect("annotated comment");

    // Read back through a fresh scan (the listing re-parses the page).
    let read = rig.review.comments(review_id).await.unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].annotation, vec![stroke], "strokes survive the page");
    assert_eq!(read[0].timecode_secs, 7.25);
    assert_eq!(drawn.id, read[0].id);

    rig.backend.shutdown().await;
}

/// Browsing is a read: `find` never mints an entity, and the review
/// only comes to exist when feedback starts (`for_file`). Once it
/// exists, `find` resolves it.
#[tokio::test(flavor = "multi_thread")]
async fn finding_a_review_never_creates_one() {
    let (_dir, rig, root_id) = rig().await;

    // A miss, twice — and nothing minted by looking.
    for _ in 0..2 {
        assert_eq!(rig.review.find(root_id, cut()).await.unwrap(), None);
    }
    assert!(
        rig.review.reviews(Some(root_id)).await.unwrap().is_empty(),
        "looking must not create"
    );

    let review = rig.review.for_file(root_id, cut()).await.unwrap();
    let found = rig
        .review
        .find(root_id, cut())
        .await
        .unwrap()
        .expect("exists now");
    assert_eq!(found.id, review.id);

    rig.backend.shutdown().await;
}

/// The refusals: an untracked file has no review, a comment must name
/// a real version, an empty comment is nothing, and a region that is
/// not a moment in the media cannot anchor one.
///
/// The legacy surface also refused a NaN timecode. The v2 region is
/// whole milliseconds, so a NaN is unrepresentable on the wire; the
/// equivalent refusal is a region the comment model cannot carry.
#[tokio::test(flavor = "multi_thread")]
async fn review_refusals_are_clean_errors() {
    let (_dir, rig, root_id) = rig().await;

    let err = rig
        .review
        .for_file(root_id, RootPath::parse("nope.mov").unwrap())
        .await
        .expect_err("untracked file");
    let text = fault_text(err);
    assert!(text.contains("not tracked"), "{text}");

    let review = rig.review.for_file(root_id, cut()).await.unwrap();
    let review_id = ReviewId::new(review.id);
    let err = rig
        .review
        .comment(comment_at(
            review_id,
            1_000,
            "ghost",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        ))
        .await
        .expect_err("unknown commit");
    assert!(!fault_text(err).is_empty());

    let head = rig.chain(root_id).await[0].commit_id.clone();
    let err = rig
        .review
        .comment(comment_at(review_id, 1_000, "   ", &head))
        .await
        .expect_err("empty comment");
    let text = fault_text(err);
    assert!(text.contains("text or a drawing"), "{text}");
    let err = rig
        .review
        .comment(NewComment {
            region: Region::Page { page: 4 },
            ..comment_at(review_id, 0, "x", &head)
        })
        .await
        .expect_err("a page is not a moment");
    let text = fault_text(err);
    assert!(text.contains("anchors to a moment"), "{text}");

    rig.backend.shutdown().await;
}
