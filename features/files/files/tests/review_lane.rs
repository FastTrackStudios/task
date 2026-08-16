//! `ReviewService` — the guest lane, against a real `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, for the reason
//! `roots_lane.rs` gives: the lane has no `permits.rs` row yet, so it is
//! not mounted on a router and cannot be reached over vox. These call the
//! trait directly, which is what a dispatcher would do.
//!
//! Two things are pinned here that are not ordinary lane behaviour:
//!
//! - the methods this lane **cannot** implement faithfully fault with
//!   `Internal("not yet implemented: …")` rather than approximating, and
//!   a test holds each of them to that so the gap cannot be closed by
//!   accident with a fake;
//! - the guest is anonymous, so what a comment is attributed to and what
//!   version it is attached to are checked at the boundary rather than
//!   trusted from the caller.

use files::FilesBackend;
use files::lane::review::served_rendition;
use files_proto::error::FilesFault;
use files_proto::id::{CommentId, ReviewId, RootId, ShareId, VersionId};
use files_proto::model::{RenditionKind, RootFlavor};
use files_proto::path::RootPath;
use files_proto::service::access::{Capability, ShareLink};
use files_proto::service::legacy::FilesService;
use files_proto::service::media::Region;
use files_proto::service::review::{NewComment, ReviewService};

/// A media root holding one checkpointed video, which is the minimum a
/// review needs: `for_file` refuses a path the checkpoint head does not
/// track, because an untracked file has no versions to review.
async fn rig() -> (tempfile::TempDir, FilesBackend, RootId) {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).expect("backend");
    let root_dir = dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("cut.mov"), vec![0x11u8; 2048]).unwrap();

    let root = FilesService::create_root(
        &backend,
        root_dir.to_string_lossy().into_owned(),
        "session".into(),
        RootFlavor::Media,
    )
    .await
    .expect("create root");
    FilesService::checkpoint_now(&backend, root.id, None)
        .await
        .expect("checkpoint");
    (dir, backend, RootId::new(root.id))
}

fn path(raw: &str) -> RootPath {
    RootPath::parse(raw).expect("path")
}

/// The head version of `cut.mov`, as the client watching it would name it.
async fn head_version(backend: &FilesBackend, root_id: RootId) -> (VersionId, String) {
    let chain = FilesService::chain(backend, root_id.get(), "cut.mov".into())
        .await
        .expect("chain");
    let commit = chain[0].commit_id.clone();
    (VersionId::from_commit_hex(&commit), commit)
}

fn link(root_id: RootId, capabilities: Vec<Capability>) -> ShareLink {
    ShareLink {
        id: ShareId::generate(),
        root_id,
        path: path("cut.mov"),
        capabilities,
        token: uuid::Uuid::new_v4().simple().to_string(),
        password_set: false,
        expires_at: None,
        disabled: false,
    }
}

// ── The member side ────────────────────────────────────────────────

/// `for_file` is the member's entry point, and it is get-or-create: one
/// review per `(root, file)`, so two people opening the same cut land in
/// the same conversation rather than each starting their own.
#[tokio::test(flavor = "multi_thread")]
async fn for_file_gets_or_creates_one_review_per_file() {
    let (_tmp, backend, root_id) = rig().await;

    let first = backend
        .for_file(root_id, path("cut.mov"))
        .await
        .expect("create the review");
    let second = backend
        .for_file(root_id, path("cut.mov"))
        .await
        .expect("find the same review");

    assert_eq!(first.id, second.id, "one review per (root, file)");
    assert_eq!(first.title, "cut.mov");
    assert_eq!(
        backend.review(ReviewId::new(first.id)).await.unwrap().id,
        first.id,
        "and it is readable by id afterwards"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_root_is_a_typed_fault_rather_than_prose() {
    let (_tmp, backend, _) = rig().await;
    let ghost = RootId::generate();
    match backend
        .for_file(ghost, path("cut.mov"))
        .await
        .expect_err("no such root")
    {
        FilesFault::RootNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_review_is_named_in_the_fault() {
    let (_tmp, backend, _) = rig().await;
    let ghost = ReviewId::generate();
    match backend.review(ghost).await.expect_err("no such review") {
        FilesFault::ReviewNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected ReviewNotFound, got {other:?}"),
    }
    // And an unknown review must not read as a real one with nothing said
    // about it yet — an empty list here would be indistinguishable from a
    // fresh review to every client.
    assert!(
        backend.comments(ghost).await.is_err(),
        "comments on a review that does not exist is a fault, not an empty page"
    );
}

// ── Comments belong to their version ───────────────────────────────

// t[verify files.index.regions]
/// A comment is attached to the version it was made against, and a new
/// version landing does not re-point it.
#[tokio::test(flavor = "multi_thread")]
async fn a_comment_stays_attached_to_the_version_it_was_made_against() {
    let (tmp, backend, root_id) = rig().await;
    let review = backend.for_file(root_id, path("cut.mov")).await.unwrap();
    let (v1, v1_hex) = head_version(&backend, root_id).await;

    let placed = backend
        .comment(NewComment {
            review: ReviewId::new(review.id),
            version: v1,
            region: Region::Time {
                start_ms: 12_500,
                end_ms: 13_000,
            },
            body: "logo lands too early".into(),
            strokes: Vec::new(),
            author: "Sam".into(),
        })
        .await
        .expect("comment on v1");

    assert_eq!(placed.commit_id, v1_hex, "records the version it was made on");
    assert!(
        (placed.timecode_secs - 12.5).abs() < f64::EPSILON,
        "and the region it was placed at"
    );

    // A new version of the file lands…
    std::fs::write(tmp.path().join("session/cut.mov"), vec![0x22u8; 4096]).unwrap();
    FilesService::checkpoint_now(&backend, root_id.get(), None)
        .await
        .expect("second checkpoint");
    let (v2, v2_hex) = head_version(&backend, root_id).await;
    assert_ne!(v2, v1, "the checkpoint really did make a new version");

    let after = backend.comments(ReviewId::new(review.id)).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].commit_id, v1_hex,
        "a new version must not silently re-point older feedback"
    );
    assert_ne!(after[0].commit_id, v2_hex);
    assert_eq!(
        after[0].review_id, review.id,
        "and the review survives its file gaining a version"
    );
}

/// Comments read in playhead order, and within one moment oldest first —
/// so a thread of replies at 0:12 reads down the page with the newest
/// last.
#[tokio::test(flavor = "multi_thread")]
async fn comments_read_in_the_order_a_player_wants_them() {
    let (_tmp, backend, root_id) = rig().await;
    let review = ReviewId::new(backend.for_file(root_id, path("cut.mov")).await.unwrap().id);
    let (version, _) = head_version(&backend, root_id).await;

    for (ms, body) in [(30_000u64, "third"), (1_000, "first"), (12_000, "second")] {
        backend
            .comment(NewComment {
                review,
                version,
                region: Region::Time {
                    start_ms: ms,
                    end_ms: ms + 500,
                },
                body: body.into(),
                strokes: Vec::new(),
                author: "Sam".into(),
            })
            .await
            .expect("comment");
    }

    let bodies: Vec<_> = backend
        .comments(review)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.body)
        .collect();
    assert_eq!(bodies, vec!["first", "second", "third"]);
}

/// The visitor is anonymous, and the boundary says so: whatever name is
/// typed, a link holder cannot post as an org member, and the comment
/// carries that it arrived through a link.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_cannot_post_as_an_org_member() {
    let (_tmp, backend, root_id) = rig().await;
    let review = ReviewId::new(backend.for_file(root_id, path("cut.mov")).await.unwrap().id);
    let (version, _) = head_version(&backend, root_id).await;

    let posted = backend
        .comment(NewComment {
            review,
            version,
            region: Region::Whole,
            body: "looks good".into(),
            // The name of a real member of the org.
            author: "Cody".into(),
            strokes: Vec::new(),
        })
        .await
        .expect("comment");

    assert_eq!(posted.author, "Cody (guest)");
    assert!(
        !posted.via_link.is_empty(),
        "an empty via_link means 'an org member wrote this' — a guest comment \
         must never read that way"
    );
}

// t[verify files.index.regions]
/// A region the comment model cannot carry is refused rather than
/// flattened onto 0:00, where it would look like it worked.
#[tokio::test(flavor = "multi_thread")]
async fn a_region_with_no_timecode_is_refused() {
    let (_tmp, backend, root_id) = rig().await;
    let review = ReviewId::new(backend.for_file(root_id, path("cut.mov")).await.unwrap().id);
    let (version, _) = head_version(&backend, root_id).await;

    let err = backend
        .comment(NewComment {
            review,
            version,
            region: Region::Page { page: 4 },
            body: "typo in the header".into(),
            strokes: Vec::new(),
            author: "Sam".into(),
        })
        .await
        .expect_err("a page is not a moment");
    assert!(matches!(err, FilesFault::Invalid(_)), "got {err:?}");
}

// ── What a link permits ────────────────────────────────────────────

// t[verify files.access.internal-sharing]
/// The resolution `scope` would perform, driven by an explicit link —
/// which is the only form in which this lane can answer it, since no
/// token reaches the trait.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_resolves_to_the_one_review_it_scopes_to() {
    let (_tmp, backend, root_id) = rig().await;
    let review = backend.for_file(root_id, path("cut.mov")).await.unwrap();

    let scope = backend
        .guest_scope(&link(
            root_id,
            vec![Capability::Read, Capability::Comment],
        ))
        .await
        .expect("scope");
    assert_eq!(scope.review, ReviewId::new(review.id), "one review, and no other");
    assert!(scope.can_comment);
    assert!(!scope.can_download, "download is not implied by commenting");
}

// t[verify files.scale.large-media]
/// Withholding download is a rule about which bytes are served, and it is
/// answered from the scope rather than from a caller's good intentions.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_without_download_is_served_a_proxy_rather_than_the_source() {
    let (_tmp, backend, root_id) = rig().await;
    backend.for_file(root_id, path("cut.mov")).await.unwrap();

    let viewer = backend
        .guest_scope(&link(root_id, vec![Capability::Read]))
        .await
        .expect("scope");
    assert_eq!(served_rendition(&viewer), Some(RenditionKind::Proxy720));

    let downloader = backend
        .guest_scope(&link(
            root_id,
            vec![Capability::Read, Capability::Download],
        ))
        .await
        .expect("scope");
    assert_eq!(
        served_rendition(&downloader),
        None,
        "a link that conveys download gets the source"
    );
}

/// Revocation binds on the next request: the link is re-resolved every
/// time rather than snapshotted, which is what makes an owner's change
/// retroactive for an already-connected guest.
#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_or_expired_link_conveys_nothing() {
    let (_tmp, backend, root_id) = rig().await;
    backend.for_file(root_id, path("cut.mov")).await.unwrap();

    let mut disabled = link(root_id, vec![Capability::Read]);
    disabled.disabled = true;
    assert!(matches!(
        backend.guest_scope(&disabled).await.expect_err("disabled"),
        FilesFault::Denied { .. }
    ));

    let mut lapsed = link(root_id, vec![Capability::Read]);
    lapsed.expires_at = Some(chrono::Utc::now() - chrono::TimeDelta::seconds(1));
    assert!(matches!(
        backend.guest_scope(&lapsed).await.expect_err("expired"),
        FilesFault::Denied { .. }
    ));
}

/// A guest lane that could create vault pages would be a guest lane that
/// can write to the vault. A link on a file with no review is refused,
/// not quietly turned into a `for_file`.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_never_creates_the_review_it_points_at() {
    let (_tmp, backend, root_id) = rig().await;
    let err = backend
        .guest_scope(&link(root_id, vec![Capability::Read]))
        .await
        .expect_err("no review exists yet");
    assert!(matches!(err, FilesFault::Invalid(_)), "got {err:?}");
    assert!(
        FilesService::list_reviews(&backend, Some(root_id.get()))
            .await
            .unwrap()
            .is_empty(),
        "and asking must not have created one"
    );
}

// ── What is honestly not implemented ───────────────────────────────

/// Pins the three methods this lane cannot answer faithfully. Each is a
/// fault naming what is missing rather than an approximation, and each
/// stays that way until the thing it names exists.
#[tokio::test(flavor = "multi_thread")]
async fn the_unimplementable_methods_say_so_rather_than_faking_it() {
    let (_tmp, backend, root_id) = rig().await;
    let review = ReviewId::new(backend.for_file(root_id, path("cut.mov")).await.unwrap().id);
    let (version, _) = head_version(&backend, root_id).await;

    // No token reaches the trait, so the link cannot be named.
    let scope = ReviewService::scope(&backend).await.expect_err("no link");
    assert!(
        matches!(&scope, FilesFault::Internal(m) if m.contains("not yet implemented")),
        "got {scope:?}"
    );

    // A ticket nothing can redeem is worse than no ticket: it fails as a
    // broken player rather than as an unimplemented method.
    let playback = backend
        .playback(review, version)
        .await
        .expect_err("no byte lane");
    match &playback {
        FilesFault::Internal(m) => assert!(
            m.contains("byte lane"),
            "the fault must name the byte lane: {m}"
        ),
        other => panic!("expected Internal, got {other:?}"),
    }

    // "One's own" needs an identity, and a link identifies a review
    // rather than a person — deleting on the id alone would let any link
    // holder remove an org member's feedback.
    let deleted = backend
        .delete_comment(CommentId::generate())
        .await
        .expect_err("no guest identity");
    assert!(
        matches!(&deleted, FilesFault::Internal(m) if m.contains("not yet implemented")),
        "got {deleted:?}"
    );
}
