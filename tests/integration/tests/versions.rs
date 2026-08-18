//! Chapter sixteen — what a file used to be, and two people at once.
//!
//! `scenario.album.restore` and `scenario.album.diverge`, which are the
//! two version stories a studio hits weekly: a session from a year ago
//! that has to open, and two engineers who saved the same file without
//! knowing about each other.
//!
//! # Both are about *not* losing things
//!
//! Restoring is non-destructive — it produces a new version and discards
//! nothing — and diverging keeps both sides rather than picking one.
//! Neither of those is the behaviour you get by default from a
//! filesystem, and both are the reason the version store exists, so both
//! deserve a test that would fail if someone made them convenient.
//!
//! # The advisory lock is advisory
//!
//! `files.concurrency.advisory-lock` says a second engineer is *told*
//! before starting and *not blocked*. Those are one rule, and testing
//! only the first half would leave the obvious wrong implementation — a
//! lock that refuses — passing.

use files::id::VersionId;
use files::path::RootPath;
use files::service::version::Resolution;

use integration::scenario::{self, Scenario};

fn take() -> RootPath {
    RootPath::parse("Audio Files/vox.wav").expect("a path")
}

/// Save over the take and pin it, twice, so there is a history.
async fn revise(s: &Scenario, body: &[u8], why: &str) {
    let file = s.orgs.acme.tree().join("Song").join("Audio Files/vox.wav");
    std::fs::write(&file, body).expect("save the take");
    scenario::pin(&s.orgs.acme, s.acme_root, why).await;
}

// t[verify files.version.restore]
/// A past version opens, and restoring loses nothing.
///
/// The assertion that matters is the *count*: after a restore the chain
/// is longer, not shorter. A restore that rewound the history would pass
/// a "does the old content come back" check and quietly destroy the work
/// done since.
#[tokio::test]
async fn restoring_a_past_version_discards_nothing() {
    let s = Scenario::open().await;
    revise(&s, b"vox take two", "comp'd the vocal").await;
    revise(&s, b"vox take three", "tuned").await;

    let version = s.as_alice().await.version().await;
    let before = version
        .chain(s.acme_root, take())
        .await
        .expect("the take has a history");
    assert!(
        before.len() >= 2,
        "expected a history to restore from, got {}",
        before.len()
    );

    // The oldest entry in the chain — the take as it was.
    let oldest = before.last().expect("a first version");
    let restored = version
        .restore(
            s.acme_root,
            take(),
            VersionId::from_commit_hex(&oldest.commit_id),
        )
        .await
        .expect("restore the take");

    let after = version
        .chain(s.acme_root, take())
        .await
        .expect("history after restoring");
    assert!(
        after.len() > before.len(),
        "restoring must produce a new version and discard nothing: \
         {} before, {} after",
        before.len(),
        after.len()
    );
    assert!(
        after.iter().any(|e| e.commit_id == restored.commit_id),
        "the restore's own version is missing from the chain"
    );
    // Every version that existed before is still reachable.
    for old in &before {
        assert!(
            after.iter().any(|e| e.commit_id == old.commit_id),
            "restoring dropped {} from the history",
            old.commit_id
        );
    }
}

// t[verify files.concurrency.advisory-lock]
/// A second engineer is told, and is not stopped.
///
/// The not-stopped half is the one worth having: a lock that refused the
/// second caller would satisfy "told before starting" and break the
/// rule, and it is the implementation someone reaches for first.
#[tokio::test]
async fn a_second_engineer_is_warned_and_not_blocked() {
    let s = Scenario::open().await;

    // Alice opens the take.
    s.as_alice()
        .await
        .version()
        .await
        .hold(s.acme_root, take())
        .await
        .expect("Alice signals that she has it open");

    // Sam looks before starting, and is told something is open.
    let sam = integration::client::Session::open(&s.orgs.acme, s.people.sam.token.clone()).await;
    let who = sam
        .version()
        .await
        .occupancy(s.acme_root, take())
        .await
        .expect("Sam may ask who has this open");
    assert!(
        !who.is_empty(),
        "Sam was not told that the take is open at all"
    );

    // And is not stopped.
    sam.version()
        .await
        .hold(s.acme_root, take())
        .await
        .expect("an advisory lock advises — it does not refuse");
}

/// **The advisory lock cannot tell two people apart.**
///
/// `hold` records `this_principal()` — the *process's* principal — so
/// every hold on a server is the same holder whoever signed the call.
/// `occupancy` says as much where it is implemented: "without a caller
/// identity on this surface the server cannot say which one is else".
///
/// So what the test above really establishes is that somebody has it
/// open and that a second caller is not blocked. The half of
/// `files.concurrency.advisory-lock` that names *who* — the half that
/// makes the warning act on-able, since "someone" and "Sam, twenty
/// minutes ago" are different messages — is not reachable yet.
///
/// This is the same identity gap `people.rs` describes for the access
/// lane's owner shortcut, in a lane that has not closed it. Asserted
/// deliberately, so closing it is a decision someone makes and sees fail
/// here rather than a surprise nobody wrote down.
#[tokio::test]
async fn two_people_holding_one_file_are_recorded_as_one() {
    let s = Scenario::open().await;

    let hers = s
        .as_alice()
        .await
        .version()
        .await
        .hold(s.acme_root, take())
        .await
        .expect("Alice holds");
    let sam = integration::client::Session::open(&s.orgs.acme, s.people.sam.token.clone()).await;
    let his = sam
        .version()
        .await
        .hold(s.acme_root, take())
        .await
        .expect("Sam holds");

    assert_eq!(
        hers.principal, his.principal,
        "if this fails, the hold lane learned who the caller is — good, \
         and this test should become an assertion that they differ"
    );
    assert_ne!(
        s.people.alice.subject, s.people.sam.subject,
        "the two are different people, which is what makes the above a gap"
    );
}

// t[verify files.version.keep-both]
/// Two saves of one file survive as two versions, and a human picks.
///
/// `seed_divergent_file` is the backend's own way of producing the state
/// two offline machines produce — the point here is not how it arose but
/// that both sides are still there afterwards, and that settling it is
/// something a person does rather than something that happened.
#[tokio::test]
async fn two_saves_of_one_file_both_survive() {
    let s = Scenario::open().await;

    s.orgs
        .acme
        .backend
        .seed_divergent_file(
            s.acme_root.get(),
            "Audio Files/vox.wav",
            b"vox, comped on the laptop",
            b"vox, comped in the studio",
        )
        .await
        .expect("two machines saved the same take");

    let version = s.as_alice().await.version().await;
    let open = version
        .divergences(s.acme_root)
        .await
        .expect("list divergences");
    let divergence = open
        .iter()
        .find(|d| d.path == "Audio Files/vox.wav")
        .expect("the take diverged and nothing merged it away");

    assert!(
        divergence.sides.len() >= 2,
        "a divergence with fewer than two sides is not one: {divergence:?}"
    );

    // A divergence is named by any of its sides — the lane finds the
    // one this commit belongs to. "Mine" is the journal line whichever
    // side you name, so naming the first is not a choice about outcome.
    let side = VersionId::from_commit_hex(&divergence.sides[0].commit_id);

    // Settling is a decision, and "keep both" is the one that loses
    // nothing — which is why it is the one with names in it.
    version
        .resolve_divergence(
            s.acme_root,
            side,
            // Empty names on purpose: caller-chosen names are not
            // implemented, and the lane says so rather than silently
            // ignoring them. Empty accepts the server's own
            // `<stem> (divergent n).<ext>` naming, which is the part of
            // `files.version.keep-both` that exists — both sides land,
            // neither overwrites the other.
            Resolution::KeepBoth {
                mine: String::new(),
                theirs: String::new(),
            },
        )
        .await
        .expect("a human keeps both");

    let left = version
        .divergences(s.acme_root)
        .await
        .expect("list divergences again");
    assert!(
        !left.iter().any(|d| d.path == "Audio Files/vox.wav"),
        "the divergence is still open after being settled: {left:?}"
    );
}
