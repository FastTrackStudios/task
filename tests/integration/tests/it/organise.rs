//! Chapter seventeen — the album, arranged by hand.
//!
//! `scenario.album.organise`: takes tagged `keeper` appear in a view
//! without moving on disk, a favourite is per-person, and the activity
//! feed says who did what.
//!
//! # Tagging is not filing
//!
//! The whole value of a tag here is that the file stays where the DAW
//! put it. A studio's folder structure is load-bearing — Reaper resolves
//! media by path, and the archive this example was drawn from has six
//! thousand folders somebody arranged on purpose — so an organiser that
//! moved things would be an organiser nobody could use. That is why the
//! assertion below checks the tag *and* the tree.

use files::path::RootPath;
use files::service::organise::Tag;

use integration::client::Session;
use integration::scenario::Scenario;

fn take() -> RootPath {
    RootPath::parse("Audio Files/vox.wav").expect("a path")
}

// t[verify files.organise.manual]
// t[verify scenario.album.organise] — takes tagged `keeper` appear in a
// view without moving on disk
/// A tagged take is findable, and has not moved.
#[tokio::test]
async fn tagging_a_take_files_it_without_moving_it() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let before = alice
        .tree()
        .await
        .browse(s.acme_root, RootPath::parse("Audio Files").unwrap())
        .await
        .expect("browse the takes");

    alice
        .organise()
        .await
        .set_tags(s.acme_root, take(), vec![Tag("keeper".into())])
        .await
        .expect("tag the take");

    let view = alice
        .organise()
        .await
        .tagged(vec![Tag("keeper".into())], Some(s.acme_root))
        .await
        .expect("the view a tag produces");
    assert!(
        view.iter().any(|m| m.path == take()),
        "the tagged take is not in the tag's view: {view:?}"
    );

    let after = alice
        .tree()
        .await
        .browse(s.acme_root, RootPath::parse("Audio Files").unwrap())
        .await
        .expect("browse again");
    assert_eq!(
        before.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        after.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        "tagging moved something on disk — the folder is load-bearing and \
         must be exactly as the DAW left it"
    );
}

// t[verify files.organise.manual]
/// A favourite round-trips for the person who set it.
#[tokio::test]
async fn a_favourite_is_remembered() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let marked = alice
        .organise()
        .await
        .set_favourite(s.acme_root, take(), true)
        .await
        .expect("Alice stars the take");
    assert!(marked.favourite);

    let read_back = alice
        .organise()
        .await
        .marks(s.acme_root, take())
        .await
        .expect("read the marks back");
    assert!(
        read_back.favourite,
        "a star that does not survive the next call is not a shortlist"
    );
}

/// **A favourite is not per-person over the wire.**
///
/// `set_favourite` keys the shortlist on `this_principal()` — the
/// *process's* principal — so on a server every caller shares one
/// shortlist. The lane's own docs name this: nothing on these traits
/// carries a caller.
///
/// `files.organise.manual` says "a favourite is per-person", and the
/// storage is already keyed by principal, so the gap is entirely in
/// which principal arrives. It is the same identity gap `people.rs`
/// describes for the access lane and `versions.rs` for the advisory
/// lock — one cause, three lanes.
///
/// Asserted deliberately, so closing it fails here rather than
/// surprising someone.
#[tokio::test]
async fn one_persons_shortlist_is_currently_everyones() {
    let s = Scenario::open().await;

    s.as_alice()
        .await
        .organise()
        .await
        .set_favourite(s.acme_root, take(), true)
        .await
        .expect("Alice stars the take");

    let sam = Session::open(&s.orgs.acme, s.people.sam.token.clone()).await;
    let his = sam
        .organise()
        .await
        .marks(s.acme_root, take())
        .await
        .expect("Sam reads the marks");

    assert!(
        his.favourite,
        "if this fails, the organise lane learned who the caller is — good, \
         and this test should become an assertion that Sam sees no star"
    );
}

// t[verify files.organise.activity]
/// The feed records that the tree changed, and when.
///
/// What it does **not** record is who, or which path — see the test
/// below. So this asserts the half that exists: a structural change
/// produces a row, ordered, with a time on it.
#[tokio::test]
async fn the_activity_feed_records_that_the_tree_changed() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let before = alice
        .organise()
        .await
        .activity(s.acme_root, None, None)
        .await
        .expect("read the feed");

    alice
        .write()
        .await
        .rename(s.acme_root, take(), "vox-comp.wav".into())
        .await
        .expect("rename the take");

    let after = alice
        .organise()
        .await
        .activity(s.acme_root, None, None)
        .await
        .expect("read the feed again");

    assert!(
        after.len() > before.len(),
        "a rename produced no activity row: {before:?} → {after:?}"
    );
    // Newest first, which is what a feed means.
    assert!(
        after.windows(2).all(|w| w[0].at >= w[1].at),
        "the feed is not in newest-first order: {after:?}"
    );
}

/// **The feed says something happened, not who did it or to what.**
///
/// Two gaps, both stated in the organise lane's own module docs and
/// neither closable there:
///
/// - **No actor.** The journal the feed is derived from has no actor
///   field, and no method on these traits carries a caller, so rows
///   report `UNATTRIBUTED` rather than stamping this process's principal
///   onto history it did not make. That is the right call and it leaves
///   "who renamed this" unanswerable.
/// - **No path.** Rows come out at the root, so "renamed *what*" is not
///   in them either.
///
/// `scenario.album.organise` wants "the album's activity feed shows who
/// renamed, uploaded and deleted what, and when". Today it shows *when*.
/// Closing this needs an actor on the journal, not more code in the
/// organise lane.
#[tokio::test]
async fn the_feed_cannot_yet_say_who_or_what() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    alice
        .write()
        .await
        .rename(s.acme_root, take(), "vox-comp.wav".into())
        .await
        .expect("rename the take");

    let feed = alice
        .organise()
        .await
        .activity(s.acme_root, None, None)
        .await
        .expect("read the feed");
    let row = feed.first().expect("at least one row");

    assert_eq!(
        row.actor,
        files::id::PrincipalId::new(uuid::Uuid::nil()),
        "if this fails, the journal carries an actor — good, and this test \
         should become an assertion that the row names Alice"
    );
    assert!(
        row.from.is_none(),
        "if this fails, rows carry the path a rename came from — good, and \
         this test should assert it is `Audio Files/vox.wav`"
    );
}
