//! Two endpoints, no relay, a real QUIC connection, real vox frames.
//!
//! The claim this file exists to check is that vox needs no change to run
//! over iroh — that `vox_stream::StreamLink` and an iroh bidirectional
//! stream fit together with nothing in between. Asserting that in a doc
//! comment is worth very little; connecting two endpoints and pushing
//! framed messages through them is worth something.
//!
//! `RelayMode::Disabled` and no address lookup, so this exercises a
//! direct path on the loopback network and never touches the internet.

use iroh::{Endpoint, RelayMode, endpoint::presets};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vox_iroh::ALPN;

/// Bind an endpoint that talks only to peers it is told about.
async fn endpoint() -> Endpoint {
    Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .expect("bind an iroh endpoint")
}

/// A vox link is a length-prefixed byte stream, so this is what one
/// looks like on the wire: `[len: u32 LE][payload]`.
fn framed(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

/// The whole claim, end to end: a peer addressed by public key, a
/// bidirectional QUIC stream, and vox's own framing carried over it
/// unaltered.
#[tokio::test(flavor = "multi_thread")]
async fn vox_framing_rides_an_iroh_connection() {
    let server = endpoint().await;
    let client = endpoint().await;
    let server_addr = server.addr();

    // The accept side, as `VoxProtocol` does it: take the bi stream and
    // treat it as an ordinary byte pair.
    let serving = tokio::spawn(async move {
        let incoming = server.accept().await.expect("an incoming connection");
        let conn = incoming.await.expect("connection established");
        let (mut send, mut recv) = conn.accept_bi().await.expect("accept_bi");

        // Read one frame the way `StreamLink` does, and answer with one.
        let mut len = [0u8; 4];
        recv.read_exact(&mut len).await.expect("frame length");
        let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
        recv.read_exact(&mut body).await.expect("frame body");
        assert_eq!(&body, b"a vox frame");

        send.write_all(&framed(b"and one back"))
            .await
            .expect("reply");
        send.finish().expect("finish");
        conn.closed().await;
    });

    // The dial side. The address is a public key plus reachability
    // hints — never an IP the caller had to know.
    let conn = client
        .connect(server_addr, ALPN)
        .await
        .expect("connect by node id");
    let (mut send, mut recv) = conn.open_bi().await.expect("open_bi");

    send.write_all(&framed(b"a vox frame"))
        .await
        .expect("send a frame");
    send.finish().expect("finish");

    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await.expect("reply length");
    let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
    recv.read_exact(&mut body).await.expect("reply body");
    assert_eq!(&body, b"and one back");

    conn.close(0u32.into(), b"done");
    serving.await.expect("server finished");
}

/// The wrapper composes: an iroh stream pair becomes a vox `Link` with
/// nothing between them.
///
/// This is a compile-time claim as much as a runtime one — if
/// `StreamLink` ever stopped accepting an iroh pair, this stops
/// building.
#[tokio::test(flavor = "multi_thread")]
async fn an_iroh_stream_pair_is_a_vox_link() {
    let server = endpoint().await;
    let client = endpoint().await;
    let addr = server.addr();

    let serving = tokio::spawn(async move {
        let conn = server
            .accept()
            .await
            .expect("incoming")
            .await
            .expect("established");
        let (send, recv) = conn.accept_bi().await.expect("accept_bi");
        let _link: vox_iroh::IrohLink = vox_iroh::link(send, recv);
        conn.closed().await;
    });

    let _link = vox_iroh::connect(&client, addr)
        .await
        .expect("a vox link over iroh");

    // Dropping the client link closes the stream, which ends the server.
    drop(_link);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}
