//! Chapter fifteen — a change reaches everyone without being asked for.
//!
//! `scenario.album.rename`: renaming a song folder "appears on every
//! other connected client without a refresh — including clients on the
//! other server". This is the half of that a suite can hold still long
//! enough to assert: a second connection, subscribed, receiving what the
//! first one did.
//!
//! # What is asserted, and what is not
//!
//! `files.live.propagation` has three clauses and they live in three
//! places. The optimistic one — renders before the server acknowledges,
//! reverts if rejected — is the client's, in `files-ui`, and asserting
//! it here would mean reimplementing a UI in a test. The converge-on-
//! reconnect one is the catalogue's: a subscriber that missed events
//! catches up by reading the catalogue, which `restart.rs` already
//! covers from the other direction.
//!
//! What is left is the clause that needs two machines and a wire, which
//! is exactly what this suite has and nothing else does: **another
//! client receives it as a `FilesEvent`, without polling.**
//!
//! # Why the timeout is generous and the failure is not
//!
//! A missed event is indistinguishable from a slow one, so a test like
//! this can only ever wait and give up. Five seconds is long enough that
//! a failure means the event is not coming — the local path from
//! `publish` to a subscriber is microseconds, and everything slower than
//! that is the wire, which the rest of this suite crosses in under one.

use std::time::Duration;

use files::path::RootPath;

use integration::scenario::Scenario;

/// Subscribe on its own connection, then run `act` and wait for an
/// event that satisfies `want`.
///
/// The subscription is established *first* and given a moment to land:
/// a subscriber that attaches after the write has not missed a slow
/// event, it has missed the whole test.
async fn expect_event(
    s: &Scenario,
    want: impl Fn(&files::FilesEvent) -> bool + Send + 'static,
    act: impl std::future::Future<Output = ()>,
) -> bool {
    let watcher: files::FilesServiceStreamClient = {
        let session = s.as_alice().await;
        session.files_stream().await
    };
    let (tx, mut rx) = vox::channel::<files::FilesEvent>();
    let sub = tokio::spawn(async move { watcher.events(tx).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !sub.is_finished(),
        "the subscription ended before anything happened: {:?}",
        sub.await
    );

    act.await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        // `recv` hands back a `SelfRef`, so the event is inspected
        // through a borrow rather than owned out of the stream.
        match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Ok(Some(event))) => {
                let mut hit = false;
                let _ = event.map(|ev| hit = want(&ev));
                if hit {
                    return true;
                }
            }
            // A lull, or a decode error on somebody else's event. Both
            // are ordinary — the hub carries every root's traffic, not
            // just this test's.
            Ok(Err(_)) | Err(_) => {}
            // The stream closed. Nothing more is coming.
            Ok(Ok(None)) => return false,
        }
    }
    false
}

// t[verify files.live.propagation]
/// A rename made on one connection arrives on another.
///
/// Two connections as one person, which is the ordinary case this rule
/// is about: a laptop and a desktop, or two windows. What distinguishes
/// them is that neither asked — the second one is subscribed, and the
/// change arrives.
#[tokio::test]
async fn a_rename_reaches_a_client_that_did_not_make_it() {
    let s = Scenario::open().await;

    let arrived = expect_event(
        &s,
        |event| matches!(event, files::FilesEvent::Checkpointed(_)),
        async {
            s.as_alice()
                .await
                .write()
                .await
                .rename(
                    s.acme_root,
                    RootPath::parse("Audio Files/vox.wav").expect("a path"),
                    "vox-comp.wav".into(),
                )
                .await
                .expect("rename the take");
        },
    )
    .await;

    assert!(
        arrived,
        "a rename on one connection did not reach a subscribed second one \
         within five seconds — `files.live.propagation` says other clients \
         update without polling"
    );
}
