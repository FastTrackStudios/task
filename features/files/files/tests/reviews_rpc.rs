//! Reviews (issue #270, Phase B) at the RPC seam: the review entity,
//! timecoded comments recording their file version, and annotation
//! round-tripping — over an in-process `LocalServer`, against real
//! vault pages on disk.

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, RootFlavor};
use files_proto::{AnnotationPoint, AnnotationStroke, NewReviewComment};

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new().merge(files::files_service_layer(backend))
}

/// A media root with one video file, checkpointed.
async fn rig() -> (
    tempfile::TempDir,
    FilesBackend,
    files::FilesServiceClient,
    uuid::Uuid,
    LocalServer,
) {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).unwrap();
    let root_dir = dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("cut.mov"), vec![0x11u8; 2048]).unwrap();
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client: files::FilesServiceClient = local.establish().await.unwrap();
    let root = client
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    client.checkpoint_now(root.id, None).await.unwrap();
    (dir, backend, client, root.id, local)
}

fn comment_at(secs: f64, body: &str, commit_id: &str) -> NewReviewComment {
    NewReviewComment {
        timecode_secs: secs,
        author: "Cody".into(),
        body: body.into(),
        commit_id: commit_id.into(),
        annotation: Vec::new(),
    }
}

/// One review per file, created on first ask; comments order by
/// timecode and record the exact (normalized) version they were made
/// on — and a new version of the file keeps them attributed (AC 2).
#[tokio::test(flavor = "multi_thread")]
async fn comments_pin_their_file_version_across_new_versions() {
    let (dir, backend, client, root_id, _local) = rig().await;

    // Get-or-create: two asks, one entity.
    let review = client
        .review_for_file(root_id, "cut.mov".into())
        .await
        .expect("create review");
    let again = client
        .review_for_file(root_id, "cut.mov".into())
        .await
        .expect("same review");
    assert_eq!(review.id, again.id, "one review per (root, file)");
    assert_eq!(review.title, "cut.mov");
    assert_eq!(
        client.list_reviews(Some(root_id)).await.unwrap().len(),
        1,
        "listed once"
    );

    // Comment on version 1 — the chain's head commit.
    let v1 = &client.chain(root_id, "cut.mov".into()).await.unwrap()[0]
        .commit_id
        .clone();
    let c1 = client
        .add_review_comment(review.id, comment_at(12.5, "logo too early", v1))
        .await
        .expect("comment on v1");
    assert_eq!(&c1.commit_id, v1, "records the version it was made on");
    assert_eq!(c1.author, "Cody");

    // A new version of the file lands…
    std::fs::write(dir.path().join("session/cut.mov"), vec![0x22u8; 4096]).unwrap();
    client.checkpoint_now(root_id, None).await.unwrap();
    let chain = client.chain(root_id, "cut.mov".into()).await.unwrap();
    assert!(chain.len() >= 2, "two versions now: {}", chain.len());
    let v2 = &chain[0].commit_id;
    assert_ne!(v1, v2);

    // …and a comment on v2 joins the SAME review, while the v1 comment
    // keeps its original attribution.
    let c2 = client
        .add_review_comment(review.id, comment_at(3.0, "new cut fixes it", v2))
        .await
        .expect("comment on v2");
    assert_eq!(&c2.commit_id, v2);
    let comments = client.review_comments(review.id).await.unwrap();
    assert_eq!(comments.len(), 2);
    // Ordered by timecode, not creation: 3.0s before 12.5s.
    assert_eq!(comments[0].id, c2.id);
    assert_eq!(comments[1].id, c1.id);
    assert_eq!(
        &comments[1].commit_id, v1,
        "old comment attribution survives"
    );

    // Delete removes the page.
    client.delete_review_comment(c2.id).await.unwrap();
    assert_eq!(client.review_comments(review.id).await.unwrap().len(), 1);

    backend.shutdown().await;
}

/// A frame drawing round-trips through the vault page in normalized
/// coordinates (AC 3's persistence half).
#[tokio::test(flavor = "multi_thread")]
async fn annotations_round_trip_through_the_vault_page() {
    let (_dir, backend, client, root_id, _local) = rig().await;
    let review = client
        .review_for_file(root_id, "cut.mov".into())
        .await
        .unwrap();
    let head = client.chain(root_id, "cut.mov".into()).await.unwrap()[0]
        .commit_id
        .clone();

    let stroke = AnnotationStroke {
        points: vec![
            AnnotationPoint { x: 0.1, y: 0.2 },
            AnnotationPoint { x: 0.5, y: 0.75 },
        ],
        color: "#ff3355".into(),
        width: 0.004,
    };
    let drawn = client
        .add_review_comment(
            review.id,
            NewReviewComment {
                timecode_secs: 7.25,
                author: String::new(),
                body: "circle this".into(),
                commit_id: head,
                annotation: vec![stroke.clone()],
            },
        )
        .await
        .expect("annotated comment");

    // Read back through a fresh scan (the listing re-parses the page).
    let read = client.review_comments(review.id).await.unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].annotation, vec![stroke], "strokes survive the page");
    assert_eq!(read[0].timecode_secs, 7.25);
    assert_eq!(drawn.id, read[0].id);

    backend.shutdown().await;
}

/// Browsing is a read: `find_review` never mints an entity, and the
/// review only comes to exist when feedback starts
/// (`review_for_file`). Once it exists, `find_review` resolves it.
#[tokio::test(flavor = "multi_thread")]
async fn finding_a_review_never_creates_one() {
    let (_dir, backend, client, root_id, _local) = rig().await;

    // A miss, twice — and nothing minted by looking.
    for _ in 0..2 {
        assert_eq!(
            client.find_review(root_id, "cut.mov".into()).await.unwrap(),
            None
        );
    }
    assert!(
        client.list_reviews(Some(root_id)).await.unwrap().is_empty(),
        "looking must not create"
    );

    let review = client
        .review_for_file(root_id, "cut.mov".into())
        .await
        .unwrap();
    let found = client
        .find_review(root_id, "cut.mov".into())
        .await
        .unwrap()
        .expect("exists now");
    assert_eq!(found.id, review.id);

    backend.shutdown().await;
}

/// The refusals: an untracked file has no review, a comment must name
/// a real version, and an empty comment is nothing.
#[tokio::test(flavor = "multi_thread")]
async fn review_refusals_are_clean_errors() {
    let (_dir, backend, client, root_id, _local) = rig().await;

    let err = client
        .review_for_file(root_id, "nope.mov".into())
        .await
        .expect_err("untracked file");
    assert!(err.to_string().contains("not tracked"), "{err}");

    let review = client
        .review_for_file(root_id, "cut.mov".into())
        .await
        .unwrap();
    let err = client
        .add_review_comment(review.id, comment_at(1.0, "ghost", "abcdef012345"))
        .await
        .expect_err("unknown commit");
    assert!(!err.to_string().is_empty());

    let head = client.chain(root_id, "cut.mov".into()).await.unwrap()[0]
        .commit_id
        .clone();
    let err = client
        .add_review_comment(review.id, comment_at(1.0, "   ", &head))
        .await
        .expect_err("empty comment");
    assert!(err.to_string().contains("text or a drawing"), "{err}");
    let err = client
        .add_review_comment(review.id, comment_at(f64::NAN, "x", &head))
        .await
        .expect_err("NaN timecode");
    assert!(err.to_string().contains("timecode"), "{err}");

    backend.shutdown().await;
}
