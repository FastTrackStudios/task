//! Chapter thirteen — the client says what they think of the mix.
//!
//! A review is the surface a client gets instead of the tree: one file,
//! its versions, and somewhere to leave feedback. `people.rs` covers who
//! may reach it; this covers what happens once they do.
//!
//! # Why this chapter exists at all
//!
//! Three of these rules were implemented and verified by nothing. That
//! is a worse state than unimplemented, because the code reads as
//! finished — `files.review.anonymity` in particular is a *refusal*, and
//! a refusal nobody exercises is one line away from becoming an
//! accidental permit.
//!
//! # Over the wire, as a member
//!
//! The guest half of this lane is served over HTTP, at
//! `/org/<slug>/share/<token>/vox`, because a browser opening a link has
//! no other way in. That surface has its own test
//! (`apps/server/tests/guest_review_e2e.rs`).
//!
//! What is asserted here is the half a member reaches on the org router,
//! which is the same backend behind both — so "a page region is refused"
//! and "no caller may delete a guest comment" are established against
//! the implementation rather than against one transport's wrapper.

use files::id::VersionId;
use files::path::RootPath;
use files::service::media::Region;
use files::service::review::NewComment;

use integration::scenario::Scenario;

/// Open a review on ACME's mix, and take the version it is at.
///
/// The deliverable rather than a session file: a review is the thing
/// that leaves the building, and anchoring this chapter to `mix-v1.wav`
/// keeps it about the same file the client chapter is about.
async fn mix_review(s: &Scenario) -> (files::model::Review, VersionId) {
    let alice = s.as_alice().await;
    let review = alice
        .review()
        .await
        .for_file(
            s.acme_root,
            RootPath::parse("Deliverables/mix-v1.wav").expect("a path"),
        )
        .await
        .expect("open a review on the mix");

    // The version being watched. Adoption pinned the tree, so the head
    // is the state the mix was in when the client was sent it.
    let chain = s
        .as_alice()
        .await
        .version()
        .await
        .chain(
            s.acme_root,
            RootPath::parse("Deliverables/mix-v1.wav").unwrap(),
        )
        .await
        .expect("the mix has a history");
    // The chain speaks commit hex; a `VersionId` is that commit's
    // leading 128 bits, and `VersionId::from_commit_hex` is the one
    // conversion — see its doc on the two incompatible ones that
    // existed before it.
    let version = chain
        .first()
        .map(|entry| VersionId::from_commit_hex(&entry.commit_id))
        .expect("at least one version");
    (review, version)
}

fn note(review: files::id::ReviewId, version: VersionId, region: Region) -> NewComment {
    NewComment {
        review,
        version,
        region,
        body: "the vocal sits low here".into(),
        strokes: Vec::new(),
        author: "Casey".into(),
    }
}

// t[verify files.review.version-anchored]
/// A comment records the version that was on screen.
#[tokio::test]
async fn a_comment_belongs_to_the_version_it_was_made_against() {
    let s = Scenario::open().await;
    let (review, version) = mix_review(&s).await;

    let added = s
        .as_alice()
        .await
        .review()
        .await
        .comment(note(
            files::id::ReviewId::new(review.id),
            version,
            Region::Time {
                start_ms: 42_000,
                end_ms: 44_000,
            },
        ))
        .await
        .expect("leave a comment");

    // Not equality: a `VersionId` is the commit's leading 128 bits, and
    // the backend resolves it against the store and records the *full*
    // spelling. That is the behaviour worth pinning — the wire id and
    // the stored id have to stay convertible, since the alternative is
    // a side table mapping one to the other.
    assert!(
        added.commit_id.starts_with(&version.commit_prefix()),
        "the comment must record the version being watched, or a later \
         version silently re-points it: stored {}, watching {}",
        added.commit_id,
        version.commit_prefix()
    );
    assert!(
        (added.timecode_secs - 42.0).abs() < f64::EPSILON,
        "42s in, not the start of the file: {}",
        added.timecode_secs
    );
}

// t[verify files.review.version-anchored]
/// A region that cannot anchor is refused, not flattened.
///
/// The rule names this case exactly — "a page number against a video" —
/// and the wrong behaviour is not an error but a *success*: a comment
/// silently filed at 0:00, which reads as feedback about the opening
/// frame of something the reviewer never mentioned.
#[tokio::test]
async fn a_region_that_cannot_anchor_is_refused_rather_than_flattened() {
    let s = Scenario::open().await;
    let (review, version) = mix_review(&s).await;
    let review_id = files::id::ReviewId::new(review.id);

    for region in [
        Region::Page { page: 3 },
        Region::Bytes {
            start: 1024,
            end: 2048,
        },
    ] {
        let refused = s
            .as_alice()
            .await
            .review()
            .await
            .comment(note(review_id, version, region.clone()))
            .await;
        assert!(
            refused.is_err(),
            "{region:?} has no moment in the media to anchor to, so it \
             must be refused rather than filed at the start"
        );
    }
}

// t[verify files.review.anonymity]
/// Nobody deletes a comment through this lane.
///
/// Not "a guest may only delete their own" — there is no *their own* to
/// check. Two visitors holding one link are indistinguishable, so
/// deleting on the comment's id alone would let either of them remove an
/// org member's feedback. The lane refuses outright, and this is the
/// test that notices if someone ever narrows it to an id check instead.
#[tokio::test]
async fn no_one_may_delete_feedback_through_the_review_lane() {
    let s = Scenario::open().await;
    let (review, version) = mix_review(&s).await;

    let added = s
        .as_alice()
        .await
        .review()
        .await
        .comment(note(
            files::id::ReviewId::new(review.id),
            version,
            Region::Whole,
        ))
        .await
        .expect("leave a comment");

    let refused = s
        .as_alice()
        .await
        .review()
        .await
        .delete_comment(files::id::CommentId::new(added.id))
        .await;

    assert!(
        refused.is_err(),
        "a link identifies a review, not a person — so no caller on this \
         lane can be shown to have written what they ask to delete"
    );

    // And the comment is still there, which is the half that matters to
    // whoever wrote it.
    let comments = s
        .as_alice()
        .await
        .review()
        .await
        .comments(files::id::ReviewId::new(review.id))
        .await
        .expect("list comments");
    assert!(
        comments.iter().any(|c| c.id == added.id),
        "the refusal must leave the comment standing"
    );
}
