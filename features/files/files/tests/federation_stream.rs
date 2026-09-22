//! Streaming content across a server boundary — `files.peering.serving`.
//!
//! Two backends in one process, on separate data directories so they are
//! separate orgs, wired to each other through an in-process
//! [`RemoteFiles`] port. No iroh here: the port is the seam the transport
//! plugs into, so testing above it exercises the relay logic without a
//! network, and `tests/integration` exercises the same path over real
//! iroh-blobs.
//!
//! The rule under test is the sharp half of `files.peering.serving`: a
//! host holding none of the content still answers `read`, fetching from
//! a host that has it. The tempting wrong implementation fails here too
//! — handing back the origin's token, so the caller would redeem against
//! the wrong server.
//!
//! # What moved to `tests/integration`
//!
//! `open_relay` (the one authorization round trip) still runs here,
//! in-process, exactly as `read_offered`/`browse_offered` always have.
//! The actual bytes no longer cross this port at all — they are
//! published into the origin's federation-blobs store and fetched over
//! iroh-blobs, which needs a real endpoint on each side. `Direct` reads
//! those published bytes straight out of the origin's store instead
//! (`FilesBackend::read_relay_chunk`, the same kind of store-level seam
//! `with_version_store` is), which is enough to prove the manifest and
//! the windowing math are right; whether the *transport* actually moves
//! gigabytes is `tests/integration/tests/it/large_media.rs`'s claim.

use std::sync::Arc;

use files::lane::federation::RemoteFiles;
use files::{FilesBackend, FilesService, RootFlavor};
use files_proto::FilesFault;
use files_proto::id::RootId;
use files_proto::path::RootPath;
use files_proto::service::access::Capability;
use files_proto::service::federation::{EndpointId, FederationService, RelayManifest};
use files_proto::service::media::{ByteTicket, MediaService};

/// The transport, in-process.
///
/// Holds the origin's backend directly, so a call that would be a QUIC
/// round trip is a function call. The authorization half —
/// `open_relay`'s secret check and publish — is the real code; the byte
/// half reads what was published straight out of the origin's store
/// rather than fetching it over iroh-blobs, which is the one thing this
/// harness cannot do without a real endpoint.
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

    async fn open_relay(
        &self,
        _origin: &EndpointId,
        secret: &str,
        token: &str,
    ) -> Result<RelayManifest, FilesFault> {
        self.0
            .open_relay(secret.to_string(), token.to_string())
            .await
    }

    async fn fetch_relay(
        &self,
        _origin: &EndpointId,
        manifest: &RelayManifest,
        range: Option<(u64, u64)>,
        dest: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<(), FilesFault> {
        use tokio::io::AsyncWriteExt as _;
        let mut at = 0u64;
        let positioned: Vec<(u64, &files_proto::service::federation::RelayChunk)> = manifest
            .chunks
            .iter()
            .map(|c| {
                let start = at;
                at += c.len;
                (start, c)
            })
            .collect();
        let total = at;
        let (first, last) = range.unwrap_or((0, total.saturating_sub(1)));
        for (chunk_start, chunk) in positioned {
            let chunk_end = chunk_start + chunk.len;
            if chunk_end <= first || chunk_start > last {
                continue;
            }
            let want_start = first.max(chunk_start) - chunk_start;
            let want_end = last.min(chunk_end.saturating_sub(1)) - chunk_start;
            let bytes = self
                .0
                .read_relay_chunk(&chunk.hash, Some((want_start, want_end)))
                .await?;
            dest.write_all(&bytes)
                .await
                .map_err(|e| FilesFault::Io(format!("relay write: {e}")))?;
        }
        Ok(())
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
async fn a_multi_chunk_relay_arrives_whole_and_in_order() {
    // Several manifest chunks on purpose: a relay that reorders or drops
    // one passes every single-chunk test.
    let len = 3 * 1024 * 1024 + 4096;
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
async fn a_withdrawn_offer_stops_the_next_relay_not_one_already_open() {
    let bytes: Vec<u8> = (0..4096u32).map(|n| (n % 251) as u8).collect();
    let (_o, _r, origin, receiver, remote_root, offer) = pair(&bytes).await;

    // Authorized once, before the withdrawal — `open_relay`'s deliberate
    // trade (`files.topology.federation`): the secret is checked here
    // and not again, so this ticket keeps working after a revoke that
    // arrives later, in exchange for the bytes never crossing as a vox
    // payload at all.
    let ticket = receiver
        .read(remote_root, p("vox.wav"))
        .await
        .expect("ticket");
    origin.withdraw(offer.grant).await.expect("withdraw");

    assert_eq!(
        slurp(&receiver, &ticket.token, Some((0, 15)))
            .await
            .unwrap(),
        bytes[0..=15],
        "a ticket authorized before the withdrawal stopped working after it"
    );

    // A *new* relay is a new authorization, so this is where a
    // revocation actually lands.
    assert!(
        receiver.read(remote_root, p("vox.wav")).await.is_err(),
        "a withdrawn offer still authorized a fresh relay"
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
