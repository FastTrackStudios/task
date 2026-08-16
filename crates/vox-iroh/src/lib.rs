//! vox over iroh.
//!
//! The transport the system is meant to run on. Every client — CLI,
//! desktop, iOS, the sync daemon, another server — reaches a peer by its
//! public key rather than an address, over a QUIC connection that
//! traverses NATs and falls back to a relay only when it must. The one
//! exception is the browser, which is served by the server it is already
//! talking to and uses vox directly.
//!
//! # This needed no change to vox or to architect
//!
//! `vox_stream::StreamLink` implements vox's `Link` over any
//! `AsyncRead + AsyncWrite` pair, with 4-byte length-prefix framing. An
//! iroh bidirectional stream *is* that pair — `SendStream` writes,
//! `RecvStream` reads. So the whole of this crate is: open a stream,
//! wrap it, hand it to vox.
//!
//! That is the third capability in this stack that was already present
//! and unexercised, after `#[subscribe]` filter params and `Rx`-in-args.
//!
//! # Shape
//!
//! The same four layers iroh's own examples use:
//!
//! - **Endpoint** — QUIC and addressing. A node is a public key.
//! - **Router** — accepts connections and dispatches them by ALPN.
//! - **Protocol handler** — [`VoxProtocol`], which turns an accepted
//!   connection into a vox `Link`.
//! - **Application** — the architect services, unchanged and unaware.
//!
//! # Why one bidirectional stream per connection
//!
//! vox multiplexes its own channels over one link, with its own credit
//! accounting; QUIC would happily give us a stream per channel. Using
//! one avoids two schedulers disagreeing about which of them is
//! applying backpressure, and it keeps the wire format identical to
//! every other vox transport — a message framed the same way over TCP,
//! a Unix socket, or this.

use std::io;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tokio::sync::mpsc;
use vox_stream::StreamLink;

/// The ALPN this transport speaks.
///
/// Versioned in the name: a peer that speaks only a later revision
/// declines at the QUIC handshake rather than after a frame it cannot
/// parse.
pub const ALPN: &[u8] = b"fts/vox/1";

/// A vox link over one iroh connection.
pub type IrohLink = StreamLink<RecvStream, SendStream>;

/// Wrap an accepted or opened bidirectional stream as a vox link.
#[must_use]
pub fn link(send: SendStream, recv: RecvStream) -> IrohLink {
    // Reader first: `StreamLink::new` takes (read, write), and an iroh
    // pair comes back as (write, read).
    StreamLink::new(recv, send)
}

/// Dial a peer and open a vox link to it.
///
/// The address is a public key plus whatever hints are known about how
/// to reach it — iroh decides between a direct path and a relay, and
/// upgrades to direct when it can without the caller noticing.
pub async fn connect(endpoint: &Endpoint, peer: impl Into<EndpointAddr>) -> io::Result<IrohLink> {
    let conn = endpoint
        .connect(peer, ALPN)
        .await
        .map_err(|e| io::Error::other(format!("iroh connect: {e}")))?;
    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(|e| io::Error::other(format!("iroh open_bi: {e}")))?;
    Ok(link(send, recv))
}

/// Accepts vox connections on [`ALPN`].
///
/// Register it on an `iroh::protocol::Router`; each accepted connection
/// becomes a link on the channel returned by [`VoxProtocol::new`], which
/// the server side drives exactly as it drives a WebSocket listener's.
#[derive(Debug, Clone)]
pub struct VoxProtocol {
    links: mpsc::Sender<IrohLink>,
}

impl VoxProtocol {
    /// A handler, and the stream of links it will produce.
    ///
    /// Bounded on purpose: an unbounded queue of accepted connections is
    /// a way for a peer to make us hold memory we never agreed to. A
    /// full queue means the accept simply waits, which is what a
    /// connecting peer should experience when the server is saturated.
    #[must_use]
    pub fn new(backlog: usize) -> (Self, mpsc::Receiver<IrohLink>) {
        let (links, rx) = mpsc::channel(backlog);
        (Self { links }, rx)
    }
}

impl ProtocolHandler for VoxProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (send, recv) = connection.accept_bi().await?;
        if self.links.send(link(send, recv)).await.is_err() {
            // Nobody is serving links any more; the server is shutting
            // down. Closing is the honest answer, rather than holding a
            // connection open that nothing will ever read.
            return Ok(());
        }
        // `accept` returning drops the connection, so hold it until the
        // peer is done. The link borrows this connection's streams.
        connection.closed().await;
        Ok(())
    }
}
