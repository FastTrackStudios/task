//! `TASK_IROH_PEER_DIR` makes a bare endpoint id dialable with no
//! network to discover it over.
//!
//! Everything else in this repo dials by bare [`iroh::EndpointId`] and
//! is right to: a deployed endpoint publishes to n0's DNS as it binds,
//! and an id resolves from anywhere. The demo has no internet and the
//! suite would not wait for one, so `iroh_host` writes each endpoint's
//! address into a shared directory and reads its siblings' back out.
//!
//! That directory is the one part of the arrangement with no other test
//! over it — the integration suite seeds its address book in-process, so
//! it exercises `MemoryLookup` and never the files. Which leaves the
//! failure this test exists for: the demo's two servers come up, publish
//! nothing either can read, and every federated call answers
//! `Unavailable` for a reason no log line names.
//!
//! Deliberately **not** a test of iroh. What is asserted is that a
//! connection completes when the only thing the dialler was given is an
//! id and the only thing pointing at an address is that directory.

use std::time::Duration;

use architect::iroh_link::{self, iroh};
use task_server::iroh_host;

/// Two endpoints, one directory, and a dial by id alone.
#[tokio::test]
async fn an_endpoint_id_dials_through_the_peer_directory() {
    let dir = tempfile::tempdir().expect("peer dir");

    // The listener comes up first and writes where it is, which is the
    // ordering a demo actually has: one server binds before the other
    // exists to read anything.
    let listener_key = iroh::SecretKey::generate();
    let listener = files::bind_endpoint(listener_key, None)
        .await
        .expect("bind the listener");
    iroh_host::publish_addr(dir.path(), "listener", &listener)
        .expect("publish the listener's address");

    let serving = listener.clone();
    tokio::spawn(async move {
        iroh_link::serve_endpoint(&serving, iroh_link::lane_acceptor_fn(|_, _| Ok(()))).await;
    });

    // The dialler is bound with a book read out of that directory, and
    // is then given nothing but an id.
    let book = files::AddressBook::new();
    iroh_host::absorb_addrs(dir.path(), &book);
    let dialler = files::bind_endpoint(iroh::SecretKey::generate(), Some(book))
        .await
        .expect("bind the dialler");

    let dialled = tokio::time::timeout(
        Duration::from_secs(20),
        iroh_link::connect(&dialler, listener.id()),
    )
    .await
    .expect("the dial did not time out");

    assert!(
        dialled.is_ok(),
        "an id published to the peer directory did not resolve: {:?}",
        dialled.err()
    );
}

/// A directory with nothing in it resolves nothing, and says so by
/// failing to connect rather than by hanging or panicking.
///
/// The negative half matters more than it looks: without it, a dial that
/// succeeded through some *other* address lookup — a relay, a cached
/// route, the machine's own LAN — would make the test above pass while
/// proving nothing about the directory.
#[tokio::test]
async fn an_id_nobody_published_does_not_resolve() {
    let dir = tempfile::tempdir().expect("peer dir");
    let stranger = iroh::SecretKey::generate().public();

    let book = files::AddressBook::new();
    iroh_host::absorb_addrs(dir.path(), &book);
    let dialler = files::bind_endpoint(iroh::SecretKey::generate(), Some(book))
        .await
        .expect("bind the dialler");

    // Short on purpose. A dial with nowhere to go does not fail fast —
    // it keeps asking — so waiting longer buys no more certainty than
    // this and only pushes the test past nextest's stall budget. The
    // positive half above resolves in well under a second, so a second
    // and a half here is a wide margin, not a close one.
    let dialled = tokio::time::timeout(
        Duration::from_millis(1500),
        iroh_link::connect(&dialler, stranger),
    )
    .await;

    // Either the dial failed or it never completed. Both are the same
    // answer — nothing knows where that endpoint is — and which one you
    // get depends on whether this machine can reach a relay, which is
    // not something a test should assert about.
    let unreachable = match dialled {
        Err(_elapsed) => true,
        Ok(result) => result.is_err(),
    };
    assert!(
        unreachable,
        "an id that was never published resolved anyway — this test's \
         positive half is not measuring the peer directory"
    );
}
