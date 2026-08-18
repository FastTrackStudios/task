//! Chapter twenty-six — a phone that sends its own photos.
//!
//! `files.device.ingest` and `scenario.album.ingest`: stills shot during
//! tracking upload from the photographer's phone into the album's inbox
//! "with no per-item action, once each, across restarts".
//!
//! # Driven on the device, not over the wire
//!
//! Every other chapter here calls through the router as a signed-in
//! person, because the question is usually whether a client can reach
//! something. This one is the exception the module docs allow for: a
//! device sweeping its own camera roll is the thing under test, and the
//! sweep happens on the device. `device.rs` is the same shape and for
//! the same reason.
//!
//! # What "once each" has to survive
//!
//! Four things, and each rules out an implementation:
//!
//! - a **rename** — so not by filename, and a camera reuses
//!   `IMG_0001.JPG` every ten thousand shots;
//! - a **remount** — so not by path;
//! - a **restart** — so not in memory;
//! - a **re-registration** — so not by device id, since minting a new
//!   one is what re-registration is.
//!
//! Content is what survives all four, and the tests below are one per
//! item on that list.

use files::path::RootPath;

use integration::scenario::Scenario;

/// A phone's camera roll with `n` stills on it.
fn camera_roll(n: usize) -> (tempfile::TempDir, Vec<String>) {
    let dir = tempfile::tempdir().expect("a camera roll");
    let mut shots = Vec::new();
    for i in 0..n {
        let name = format!("IMG_{i:04}.JPG");
        // Distinct content per shot: two photos of the same thing are
        // still two photos, and identical *bytes* are the case the
        // dedup test below is about.
        std::fs::write(
            dir.path().join(&name),
            format!("still number {i}").as_bytes(),
        )
        .expect("write a still");
        shots.push(name);
    }
    (dir, shots)
}

fn inbox() -> RootPath {
    RootPath::parse("Inbox").expect("a path")
}

// t[verify files.device.ingest]
// t[verify scenario.album.ingest]
/// The stills arrive without anyone touching them one by one.
#[tokio::test]
async fn a_camera_roll_arrives_in_the_inbox_by_itself() {
    let s = Scenario::open().await;
    let (roll, shots) = camera_roll(5);

    // One action: this folder goes to that inbox. There is deliberately
    // no verb for sending an individual photo.
    let source = s.orgs.acme.backend.watch_source(
        roll.path().to_string_lossy().into_owned(),
        s.acme_root,
        inbox(),
    );

    let report = s
        .orgs
        .acme
        .backend
        .ingest_now(source.id)
        .expect("sweep the camera roll");
    assert_eq!(report.ingested.len(), 5, "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");

    // And they are where a person will look for them.
    let landed = s.orgs.acme.tree().join("Song").join("Inbox");
    for shot in &shots {
        assert!(
            landed.join(shot).exists(),
            "{shot} is not in the inbox: {:?}",
            std::fs::read_dir(&landed).map(|d| d.flatten().count())
        );
    }
}

// t[verify files.device.ingest]
/// Sweeping twice sends nothing the second time.
#[tokio::test]
async fn a_second_sweep_uploads_nothing() {
    let s = Scenario::open().await;
    let (roll, _) = camera_roll(4);
    let source = s.orgs.acme.backend.watch_source(
        roll.path().to_string_lossy().into_owned(),
        s.acme_root,
        inbox(),
    );

    let first = s.orgs.acme.backend.ingest_now(source.id).expect("sweep");
    assert_eq!(first.ingested.len(), 4);

    let second = s
        .orgs
        .acme
        .backend
        .ingest_now(source.id)
        .expect("sweep again");
    assert!(
        second.ingested.is_empty(),
        "a second sweep re-sent {:?}",
        second.ingested
    );
    assert_eq!(second.already, 4);

    // Four photos, four files. Not eight.
    let landed = s.orgs.acme.tree().join("Song").join("Inbox");
    let count = std::fs::read_dir(&landed).expect("read").flatten().count();
    assert_eq!(count, 4, "the inbox has {count} files after two sweeps");
}

// t[verify files.device.ingest]
/// A renamed photo is the same photo.
///
/// The clause that rules out remembering by filename — and a camera
/// reusing `IMG_0001.JPG` after ten thousand shots is why that matters
/// in the other direction too.
#[tokio::test]
async fn renaming_a_still_does_not_make_it_a_new_one() {
    let s = Scenario::open().await;
    let (roll, shots) = camera_roll(1);
    let source = s.orgs.acme.backend.watch_source(
        roll.path().to_string_lossy().into_owned(),
        s.acme_root,
        inbox(),
    );
    s.orgs.acme.backend.ingest_now(source.id).expect("sweep");

    // The phone renames it — an edit, a favourite, an export.
    std::fs::rename(
        roll.path().join(&shots[0]),
        roll.path().join("Sunset over the studio.jpg"),
    )
    .expect("rename");

    let again = s.orgs.acme.backend.ingest_now(source.id).expect("sweep");
    assert!(
        again.ingested.is_empty(),
        "a renamed still was sent a second time: {:?}",
        again.ingested
    );
}

// t[verify files.device.ingest]
/// Restarting the server does not re-send the roll.
///
/// "Idempotent across restarts" — so the ledger is on disk, not in the
/// process. Written as a restart of the server holding the root, which
/// is the same event from the other side: whatever remembers has to be
/// something that survives.
#[tokio::test]
async fn a_restart_does_not_re_send_what_was_already_sent() {
    let s = Scenario::open().await;
    let (roll, _) = camera_roll(3);
    let source = s.orgs.acme.backend.watch_source(
        roll.path().to_string_lossy().into_owned(),
        s.acme_root,
        inbox(),
    );
    assert_eq!(
        s.orgs
            .acme
            .backend
            .ingest_now(source.id)
            .expect("sweep")
            .ingested
            .len(),
        3
    );

    let acme = s.orgs.acme.restart().await;

    // The source is still declared — it was a decision, and decisions
    // survive — and sweeping it again sends nothing.
    let sources = acme.backend.ingest_sources();
    assert_eq!(sources.len(), 1, "the declared source did not survive");

    let after = acme.backend.ingest_now(sources[0].id).expect("sweep");
    assert!(
        after.ingested.is_empty(),
        "a restart re-sent {:?} — the ledger was in memory",
        after.ingested
    );

    let landed = acme.tree().join("Song").join("Inbox");
    assert_eq!(
        std::fs::read_dir(&landed).expect("read").flatten().count(),
        3
    );
}

// t[verify files.device.ingest]
/// A touched file is re-read and still not re-sent.
///
/// The cheap stat check is a cache, and this defeats it deliberately —
/// identical bytes rewritten, so the mtime moves and the path does not.
/// Every file is read and hashed again, and every one is recognised.
///
/// It is here because the first implementation passed
/// `a_second_sweep_uploads_nothing` and hung on exactly this: it handed
/// the store every file on every sweep to ask whether the store already
/// had it, and the third such call inside one runtime deadlocked. A
/// sweep that only looks fast when nothing has been touched is not a
/// sweep anybody can leave running.
#[tokio::test]
async fn touching_every_file_re_reads_and_still_sends_nothing() {
    let s = Scenario::open().await;
    let (roll, shots) = camera_roll(4);
    let source = s.orgs.acme.backend.watch_source(
        roll.path().to_string_lossy().into_owned(),
        s.acme_root,
        inbox(),
    );
    assert_eq!(
        s.orgs
            .acme
            .backend
            .ingest_now(source.id)
            .expect("sweep")
            .ingested
            .len(),
        4
    );

    // Identical bytes, new mtime — a sync client, a backup tool, a
    // photo app rewriting its own library.
    for (i, shot) in shots.iter().enumerate() {
        std::fs::write(
            roll.path().join(shot),
            format!("still number {i}").as_bytes(),
        )
        .expect("rewrite");
    }

    let again = s
        .orgs
        .acme
        .backend
        .ingest_now(source.id)
        .expect("sweep again");
    assert!(
        again.ingested.is_empty(),
        "touched files were re-sent: {:?}",
        again.ingested
    );
    assert_eq!(again.already, 4);
    assert_eq!(
        std::fs::read_dir(s.orgs.acme.tree().join("Song").join("Inbox"))
            .expect("read")
            .flatten()
            .count(),
        4
    );
}

/// Declaring the same arrangement twice is one source.
///
/// A device re-running its setup should not end up sweeping one card
/// into one inbox twice per pass.
#[tokio::test]
async fn declaring_a_source_twice_declares_it_once() {
    let s = Scenario::open().await;
    let (roll, _) = camera_roll(1);
    let at = roll.path().to_string_lossy().into_owned();

    let first = s
        .orgs
        .acme
        .backend
        .watch_source(at.clone(), s.acme_root, inbox());
    let again = s.orgs.acme.backend.watch_source(at, s.acme_root, inbox());

    assert_eq!(first.id, again.id);
    assert_eq!(s.orgs.acme.backend.ingest_sources().len(), 1);
}

/// A card that is not plugged in is not an error.
#[tokio::test]
async fn a_source_that_is_not_there_sweeps_to_nothing() {
    let s = Scenario::open().await;
    let source =
        s.orgs
            .acme
            .backend
            .watch_source("/nowhere/no-card-here".to_owned(), s.acme_root, inbox());

    let report = s
        .orgs
        .acme
        .backend
        .ingest_now(source.id)
        .expect("an unplugged card is the ordinary state of a card");
    assert!(report.ingested.is_empty());
    assert!(report.failed.is_empty());
}
