//! Streaming content across a server boundary — `files.peering.serving`.
//!
//! Two backends in one process, on separate data directories so they are
//! separate orgs, wired to each other through an in-process
//! [`RemoteFiles`] port. No iroh here: the port is the seam the transport
//! plugs into, so testing above it exercises the relay logic without a
//! network, and `tests/integration` exercises the same path over real
//! QUIC.
//!
//! The rule under test is the sharp half of `files.peering.serving`: a
//! host holding none of the content still answers `read`, fetching from
//! a host that has it. The tempting wrong implementations both fail
//! here — handing back the origin's token (the caller would redeem
//! against the wrong server) and buffering the object to relay it (the
//! multi-chunk test below is larger than the chunk ceiling on purpose).

use std::sync::Arc;

use files::lane::federation::RemoteFiles;
use files::{FilesBackend, FilesService, RootFlavor};
use files_proto::FilesFault;
use files_proto::id::RootId;
use files_proto::path::RootPath;
use files_proto::service::access::Capability;
use files_proto::service::federation::{ByteRange, EndpointId, FederationService};
use files_proto::service::media::{ByteTicket, MediaService};

/// The transport, in-process.
///
/// Holds the origin's backend directly, so a call that would be a QUIC
/// round trip is a function call. Everything above it — the secret
/// check, the subtree resolution, the chunk loop — is the real code.
#[derive(Debug)]
struct Direct(FilesBackend);

#[async_trait::async_trait]
impl RemoteFiles for Direct {
    async fn browse_offered(
        &self,
        _origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<Vec<files_proto::model::BrowseEntry>, FilesFault> {
        self.0
            .browse_offered(secret.to_string(), path.clone())
            .await
    }

    async fn read_offered(
        &self,
        _origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<ByteTicket, FilesFault> {
        self.0.read_offered(secret.to_string(), path.clone()).await
    }

    async fn fetch_offered(
        &self,
        _origin: &EndpointId,
        secret: &str,
        token: &str,
        range: ByteRange,
    ) -> Result<Vec<u8>, FilesFault> {
        self.0
            .fetch_offered(secret.to_string(), token.to_string(), range)
            .await
    }
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

/// An origin holding `Stems/vox.wav`, and a receiver holding nothing,
/// with the subtree already offered and accepted.
async fn pair(
    bytes: &[u8],
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    FilesBackend,
    FilesBackend,
    RootId,
    files_proto::service::federation::Offer,
) {
    let origin_dir = tempfile::tempdir().expect("origin dir");
    let origin = FilesBackend::new(origin_dir.path(), origin_dir.path().join("vault"))
        .expect("origin backend");

    let tree = origin_dir.path().join("session");
    std::fs::create_dir_all(tree.join("Stems")).unwrap();
    std::fs::write(tree.join("Stems").join("vox.wav"), bytes).unwrap();
    // A sibling that was *not* offered. It has to exist, or the escape
    // test below passes because the path is missing rather than because
    // the boundary held.
    std::fs::write(tree.join("Song.rpp"), b"REAPER project").unwrap();
    let root = origin
        .create_root(
            tree.to_string_lossy().into_owned(),
            "Session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("create root");
    let root = RootId::new(root.id);
    // The byte lane serves the checkpoint head, never the live file.
    origin
        .checkpoint_now(root.into(), None)
        .await
        .expect("checkpoint");

    let receiver_dir = tempfile::tempdir().expect("receiver dir");
    let receiver = FilesBackend::new(receiver_dir.path(), receiver_dir.path().join("vault"))
        .expect("receiver backend")
        .with_remotes("receiver", Arc::new(Direct(origin.clone())));

    let offer = origin
        .offer(
            root,
            p("Stems"),
            EndpointId("receiver".into()),
            vec![Capability::Read],
        )
        .await
        .expect("offer");
    let accepted = receiver.accept(offer.clone()).await.expect("accept");

    (
        origin_dir,
        receiver_dir,
        origin,
        receiver,
        accepted.root_id,
        offer,
    )
}

async fn slurp(
    backend: &FilesBackend,
    token: &str,
    range: Option<(u64, u64)>,
) -> Result<Vec<u8>, FilesFault> {
    let mut out = Vec::new();
    backend.redeem_bytes(token, range, &mut out).await?;
    Ok(out)
}

// t[verify files.peering.serving]
#[tokio::test]
async fn a_host_serves_bytes_it_does_not_hold() {
    let bytes = b"vox take one".to_vec();
    let (_o, _r, _origin, receiver, remote_root, _offer) = pair(&bytes).await;

    // The ordinary `read` call. The caller never learns the object is
    // somewhere else.
    let ticket = receiver
        .read(remote_root, p("vox.wav"))
        .await
        .expect("a ticket for a file on another server");
    assert_eq!(ticket.length, Some(bytes.len() as u64));
    assert_eq!(slurp(&receiver, &ticket.token, None).await.unwrap(), bytes);
}

// t[verify files.peering.serving]
#[tokio::test]
async fn the_receivers_ticket_is_its_own() {
    let (_o, _r, origin, receiver, remote_root, _offer) = pair(b"vox take one").await;
    let ticket = receiver
        .read(remote_root, p("vox.wav"))
        .await
        .expect("ticket");

    // Handing back the origin's token would make a federated file a
    // download link to another server — `files.topology.federation`
    // refuses exactly that, and the symptom would be a token that only
    // works if you happen to ask the right machine.
    assert!(
        slurp(&origin, &ticket.token, None).await.is_err(),
        "the receiver's token redeemed at the origin"
    );
}

// t[verify files.scale.large-media]
#[tokio::test]
async fn a_relay_larger_than_one_chunk_arrives_whole_and_in_order() {
    // Over the chunk ceiling on purpose: a relay that buffers the object
    // instead of streaming it passes every smaller test.
    let len = (ByteRange::MAX_LEN as usize) * 2 + 4096;
    let bytes: Vec<u8> = (0..len).map(|n| (n % 251) as u8).collect();
    let (_o, _r, _origin, receiver, remote_root, _offer) = pair(&bytes).await;

    let ticket = receiver
        .read(remote_root, p("vox.wav"))
        .await
        .expect("ticket");
    assert_eq!(ticket.length, Some(len as u64));
    let got = slurp(&receiver, &ticket.token, None).await.expect("relay");
    assert_eq!(got.len(), bytes.len(), "relay lost or duplicated a chunk");
    // Compared position-wise rather than whole: a chunk loop that
    // reorders or repeats is what this exists to catch, and the offset
    // of the first divergence says which chunk did it.
    assert_eq!(
        got.iter().zip(&bytes).position(|(a, b)| a != b),
        None,
        "relayed bytes diverge from the origin's"
    );
}

// t[verify files.peering.serving]
#[tokio::test]
async fn a_relayed_ticket_seeks() {
    // What a preview does. Scrubbing must transfer the part sought to,
    // not everything before it — which is why the relay is ranged rather
    // than a download followed by a slice.
    let bytes: Vec<u8> = (0..4096u32).map(|n| (n % 251) as u8).collect();
    let (_o, _r, _origin, receiver, remote_root, _offer) = pair(&bytes).await;

    let ticket = receiver
        .read(remote_root, p("vox.wav"))
        .await
        .expect("ticket");
    assert!(ticket.seekable);
    let window = slurp(&receiver, &ticket.token, Some((1000, 1099)))
        .await
        .expect("range");
    assert_eq!(window, bytes[1000..=1099]);
}

// t[verify files.topology.federation]
#[tokio::test]
async fn revocation_lands_mid_transfer() {
    let bytes: Vec<u8> = (0..4096u32).map(|n| (n % 251) as u8).collect();
    let (_o, _r, origin, receiver, remote_root, offer) = pair(&bytes).await;

    let ticket = receiver
        .read(remote_root, p("vox.wav"))
        .await
        .expect("ticket");
    assert!(slurp(&receiver, &ticket.token, Some((0, 15))).await.is_ok());

    // The grant ends while the receiver holds a live ticket. Checking the
    // secret once per file rather than once per chunk would mean
    // revocation takes effect "after this 244 GB finishes".
    origin.withdraw(offer.grant).await.expect("withdraw");
    assert!(
        slurp(&receiver, &ticket.token, Some((16, 31)))
            .await
            .is_err(),
        "a withdrawn grant still served the next chunk"
    );
}

// t[verify files.access.granularity]
#[tokio::test]
async fn a_relay_cannot_be_walked_out_of_its_subtree() {
    let (_o, _r, origin, receiver, remote_root, offer) = pair(b"vox take one").await;
    let root = offer.root_id;
    // `Stems` was offered; the REAPER project beside it was not, and it
    // exists — so this fails on the boundary, not on absence. The
    // receiver addresses paths relative to what it was given, so there
    // is no spelling of "the parent" for it to try.
    assert!(
        origin.read(root, p("Song.rpp")).await.is_ok(),
        "fixture: the unoffered sibling must exist for this to mean anything"
    );
    assert!(receiver.read(remote_root, p("Song.rpp")).await.is_err());
}

// t[verify files.peering.serving]
#[tokio::test]
async fn a_remote_root_does_not_report_previews_it_cannot_see() {
    let (_o, _r, _origin, receiver, remote_root, _offer) = pair(b"vox take one").await;
    // Renditions are derived on the host that holds the content, and
    // this one does not. An empty list would be the dangerous answer —
    // a UI reads that as "no preview exists for this file" and stops
    // asking, rather than as "the previews are on another server". So
    // this must fail rather than succeed emptily, until the relay
    // covers derived content too.
    let asked = MediaService::renditions(&receiver, remote_root, p("vox.wav")).await;
    assert!(
        asked.is_err(),
        "a remote root reported {:?} previews as though it had looked",
        asked.map(|r| r.len())
    );
}
