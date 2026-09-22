//! The [`RemoteFiles`] port, over iroh — `files.topology.multi-server`.
//!
//! [`crate::peer`] is the serving half: who may dial this store, and what
//! they may do once they have. This is the dialling half, and the two are
//! deliberately the same shape — an endpoint id on both ends, proved by
//! the handshake rather than presented as a claim.
//!
//! # Why an endpoint id is the whole address
//!
//! [`architect::iroh_link::bind_endpoint`] binds with the n0 preset:
//! relay servers, DNS address lookup and pkarr publishing. An endpoint
//! that has published can be dialled from anywhere by its bare
//! [`iroh::EndpointId`] — no host, no port, no certificate, and no
//! address book that has to be kept current. So [`EndpointId`] here is
//! parsed straight into an iroh id and handed to the dialler.
//!
//! That is what makes a `Remote` durable. An offer records who minted it
//! and nothing about where they were; the origin can move networks,
//! change ISP or sit behind a different NAT, and the accepted root keeps
//! resolving. An address book would have made every one of those a
//! re-registration.
//!
//! # When there is nothing to discover
//!
//! Discovery needs the network it is discovering over. Two servers on one
//! laptop with no internet publish to nobody and resolve nothing, which
//! is the ordinary case for a demo and for the integration suite. For
//! those, an endpoint may carry an [`iroh::address_lookup::MemoryLookup`]
//! seeded with addresses it already knows.
//!
//! That is a property of the *endpoint*, not of this type: seeding a
//! memory lookup is how a caller says "resolve these ids locally", and
//! everything downstream still dials by bare id. Which is the reason the
//! integration suite's address book stopped being a `HashMap` this type
//! had to consult — a code path that only existed in tests, wrapped
//! around the one call that mattered.
//!
//! # Connections are pooled
//!
//! One QUIC connection per origin, many bi-streams over it — a vox
//! connection is a stream, not a connection. Dialling per call costs a
//! handshake per chunk, and `fetch_offered` is called once per bounded
//! range of a large file, so that is a handshake per megabyte of someone
//! else's footage.
//!
//! A pooled connection can still be dead: the origin restarted, the NAT
//! rebound, the relay dropped it. `open_bi` is where that surfaces, so a
//! failure there redials once and retries before giving up — a stale
//! entry costs one extra round trip, not an error the caller has to
//! understand.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use architect::iroh_link::{self, iroh};
use files_proto::error::FilesFault;
use files_proto::model::BrowseEntry;
use files_proto::path::RootPath;
use files_proto::service::federation::{EndpointId, RelayChunk, RelayManifest};
use files_proto::service::media::ByteTicket;
use tokio::sync::Mutex;

use crate::lane::federation::RemoteFiles;

/// Addresses an endpoint already knows, for when there is nothing to
/// discover.
///
/// See the module docs: a deployment resolves ids through n0 DNS and
/// needs none of this, but two servers on one laptop with no internet
/// resolve nothing at all. Seeding this is how such a caller says
/// "resolve these ids locally" *without* anything downstream learning
/// that an address exists — every dial is still by bare id.
///
/// Cloning shares one table, so a book handed to several endpoint
/// builders can be topped up afterwards and every endpoint sees it.
pub type AddressBook = iroh::address_lookup::memory::MemoryLookup;

/// Bind an endpoint that serves and dials the vox protocol, and serves
/// (over a second ALPN) an iroh-blobs relay.
///
/// [`architect::iroh_link::bind_endpoint`] with two additions: an
/// optional [`AddressBook`], and `iroh_blobs::ALPN` alongside vox's own.
/// ALPN is negotiated once per QUIC connection, so the two never share
/// one — [`peer::serve_over_iroh`](crate::peer::serve_over_iroh)'s
/// accept loop dispatches an incoming connection to whichever protocol
/// it negotiated. The n0 preset underneath is otherwise unchanged —
/// relay, DNS lookup and pkarr publishing — so an endpoint bound with a
/// book still discovers everything it would have discovered, and merely
/// also knows what it was told.
pub async fn bind_endpoint(
    secret_key: iroh::SecretKey,
    book: Option<AddressBook>,
) -> Result<iroh::Endpoint, iroh::endpoint::BindError> {
    let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(secret_key)
        .alpns(vec![
            iroh_link::VOX_ALPN.to_vec(),
            iroh_blobs::ALPN.to_vec(),
        ]);
    if let Some(book) = book {
        builder = builder.address_lookup(book);
    }
    builder.bind().await
}

/// How long a dial may take before the origin counts as unreachable.
///
/// Without a bound, `connect` waits for an origin that is not coming
/// back — and a browse of federated content becomes a hang rather than
/// an answer. That is the one outcome `files.catalogue.offline` rules
/// out by name: unavailable content is *marked*, never missing, and a
/// surface cannot mark what it is still waiting for.
///
/// Five seconds is chosen against discovery rather than against
/// patience: resolving an id through n0's DNS and opening a QUIC
/// connection is well under a second on a working network, and the
/// interesting case here is the network that is not working. A caller
/// who would rather wait longer has a root that is not reachable, which
/// is a different problem from a slow one.
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// The [`RemoteFiles`] port, dialling other servers over iroh.
///
/// Install it on a backend with `FilesBackend::with_remotes` — or, from
/// the server, `task_server::attach_peering`, which takes `&mut` for the
/// reason documented there.
#[derive(Debug)]
pub struct IrohRemotes {
    /// The endpoint this store dials *from*. Its own id is what the far
    /// side's admitted-host list is consulted for, so this is the org's
    /// endpoint rather than a fresh one.
    endpoint: iroh::Endpoint,
    /// One live connection per origin. See the module docs on why this
    /// is pooled and how a stale entry is noticed.
    pool: Mutex<HashMap<iroh::EndpointId, iroh::endpoint::Connection>>,
    /// One live iroh-blobs connection per origin — a second pool because
    /// ALPN is negotiated per QUIC connection, so this cannot share an
    /// entry with `pool` even when it is the same origin. This is the
    /// connection `fetch_relay` reuses across every chunk and every
    /// file, which is the whole point: one handshake per origin ever,
    /// not one per megabyte.
    blobs_pool: Mutex<HashMap<iroh::EndpointId, iroh::endpoint::Connection>>,
    /// Where `fetch_relay` receives into, on disk rather than in
    /// memory — an in-memory receiver held the whole object's worth of
    /// pages by the time a big fetch finished (measured, not assumed:
    /// bounded per-window buffers still didn't bound *this*, because
    /// what a `MemStore` holds is not released the moment its handle is
    /// dropped). Writing to a file-backed store instead means every
    /// byte a window receives is on disk, not the heap, before it is
    /// read back out to hand to the caller — a page a normal write
    /// syscall touches does not count toward this process's RSS the way
    /// a `MemStore`'s pages apparently do.
    ///
    /// Held for the endpoint's lifetime (an ephemeral `TempDir`, gone
    /// at drop) rather than reopened per fetch: content-addressed, so a
    /// second fetch of a chunk this org already relayed once — from any
    /// origin — costs nothing.
    scratch: Arc<tokio::sync::OnceCell<(tempfile::TempDir, iroh_blobs::store::fs::FsStore)>>,
}

impl IrohRemotes {
    /// Dial from `endpoint`.
    #[must_use]
    pub fn new(endpoint: iroh::Endpoint) -> Self {
        Self {
            endpoint,
            pool: Mutex::new(HashMap::new()),
            blobs_pool: Mutex::new(HashMap::new()),
            scratch: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// Wrap in an `Arc` for the port, which is how every caller wants it.
    #[must_use]
    pub fn port(endpoint: iroh::Endpoint) -> Arc<dyn RemoteFiles> {
        Arc::new(Self::new(endpoint))
    }

    /// This endpoint's own id — what a peer admits to let this store in.
    #[must_use]
    pub fn endpoint_id(&self) -> String {
        self.endpoint.id().to_string()
    }

    /// Open a federation lane on `origin`, over a pooled connection.
    async fn lane(
        &self,
        origin: &EndpointId,
    ) -> Result<crate::FederationServiceClient, FilesFault> {
        let id: iroh::EndpointId = origin.0.parse().map_err(|e| {
            // A malformed id is not an outage. Saying `Unavailable` here
            // would tell the caller to retry something that cannot come
            // back.
            FilesFault::Io(format!("not an endpoint id: {} ({e})", origin.0))
        })?;

        // The pooled connection first; a fresh one if it has died. Both
        // paths end at the same `open_bi`, so a stale entry costs a
        // redial rather than an error.
        let link = match self.pooled(id).await {
            Some(connection) => match Self::open(connection).await {
                Ok(link) => link,
                Err(_) => {
                    self.pool.lock().await.remove(&id);
                    Self::open(self.redial(id).await?).await?
                }
            },
            None => Self::open(self.redial(id).await?).await?,
        };

        // `establish` does the handshake and opens the service lane in
        // one step — the same call a client makes over a WebSocket, with
        // only the link underneath it different.
        architect::vox::initiator_on(link)
            .establish()
            .await
            .map_err(|e| FilesFault::Io(format!("establish on {origin}: {e}")))
    }

    async fn pooled(&self, id: iroh::EndpointId) -> Option<iroh::endpoint::Connection> {
        self.pool.lock().await.get(&id).cloned()
    }

    /// Dial `id` and remember the connection.
    ///
    /// By bare id: the endpoint resolves it through whatever address
    /// lookup it was bound with — n0 DNS in a deployment, a seeded
    /// [`iroh::address_lookup::MemoryLookup`] where there is nothing to
    /// discover.
    async fn redial(&self, id: iroh::EndpointId) -> Result<iroh::endpoint::Connection, FilesFault> {
        let dialled =
            tokio::time::timeout(DIAL_TIMEOUT, self.endpoint.connect(id, iroh_link::VOX_ALPN))
                .await;
        // `Unavailable` rather than `Io` for the timeout: the caller's
        // reasonable response is to mark this content unreachable and
        // carry on, which is a different response from "something went
        // wrong on the wire" — and the same one an offline origin gets.
        let connection = match dialled {
            Ok(Ok(connection)) => connection,
            Ok(Err(e)) => return Err(FilesFault::Io(format!("dial {id}: {e}"))),
            Err(_elapsed) => {
                return Err(FilesFault::Unavailable {
                    path: RootPath::root(),
                });
            }
        };
        self.pool.lock().await.insert(id, connection.clone());
        Ok(connection)
    }

    /// One vox connection: one bi-stream on an existing QUIC connection.
    async fn open(
        connection: iroh::endpoint::Connection,
    ) -> Result<iroh_link::IrohLink, FilesFault> {
        let (send, recv) = connection
            .open_bi()
            .await
            .map_err(|e| FilesFault::Io(format!("open stream: {e}")))?;
        Ok(iroh_link::IrohLink::new(connection, send, recv))
    }

    /// The pooled iroh-blobs connection to `id`, dialling fresh if there
    /// is none or the pooled one has died.
    ///
    /// Mirrors `redial`/`pooled` exactly, on a separate pool: ALPN is
    /// negotiated once per QUIC connection, so a vox connection to `id`
    /// cannot double as this one. `fetch_relay` calls this once per
    /// origin, ever (until the connection dies) — every chunk after
    /// that reuses it as a fresh multiplexed stream, which is what a
    /// megabyte cost a fresh handshake for before.
    async fn blobs_connection(
        &self,
        id: iroh::EndpointId,
    ) -> Result<iroh::endpoint::Connection, FilesFault> {
        if let Some(connection) = self.blobs_pool.lock().await.get(&id).cloned()
            && connection.close_reason().is_none()
        {
            return Ok(connection);
        }
        let dialled =
            tokio::time::timeout(DIAL_TIMEOUT, self.endpoint.connect(id, iroh_blobs::ALPN)).await;
        let connection = match dialled {
            Ok(Ok(connection)) => connection,
            Ok(Err(e)) => return Err(FilesFault::Io(format!("blobs dial {id}: {e}"))),
            Err(_elapsed) => {
                return Err(FilesFault::Unavailable {
                    path: RootPath::root(),
                });
            }
        };
        self.blobs_pool.lock().await.insert(id, connection.clone());
        Ok(connection)
    }

    /// The on-disk store `fetch_relay` receives into. See the field doc
    /// on why this is a file-backed store rather than a `MemStore`.
    async fn scratch_store(&self) -> Result<&iroh_blobs::store::fs::FsStore, FilesFault> {
        self.scratch
            .get_or_try_init(|| async {
                let dir = tempfile::tempdir()
                    .map_err(|e| FilesFault::Io(format!("relay scratch dir: {e}")))?;
                let store = iroh_blobs::store::fs::FsStore::load(dir.path())
                    .await
                    .map_err(|e| FilesFault::Io(format!("opening relay scratch store: {e}")))?;
                Ok((dir, store))
            })
            .await
            .map(|(_, store)| store)
    }
}

/// Unwrap a vox error into the fault the origin actually raised.
///
/// Without this every remote refusal reaches the caller as a transport
/// error, and `Denied` — the answer a withdrawn grant is supposed to
/// give — becomes indistinguishable from the origin being unreachable.
fn fault(e: architect::vox::VoxError<FilesFault>) -> FilesFault {
    match e {
        architect::vox::VoxError::User(fault) => *fault,
        other => FilesFault::Io(other.to_string()),
    }
}

// t[impl files.topology.multi-server] — "where two peers can reach each
// other, bytes move directly over iroh/QUIC": this is the dialling half,
// and until it existed the only implementation of the port was in the
// test harness
#[async_trait::async_trait]
impl RemoteFiles for IrohRemotes {
    async fn browse_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<Vec<BrowseEntry>, FilesFault> {
        self.lane(origin)
            .await?
            .browse_offered(secret.to_string(), path.clone())
            .await
            .map_err(fault)
    }

    async fn read_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<ByteTicket, FilesFault> {
        self.lane(origin)
            .await?
            .read_offered(secret.to_string(), path.clone())
            .await
            .map_err(fault)
    }

    async fn open_relay(
        &self,
        origin: &EndpointId,
        secret: &str,
        token: &str,
    ) -> Result<RelayManifest, FilesFault> {
        self.lane(origin)
            .await?
            .open_relay(secret.to_string(), token.to_string())
            .await
            .map_err(fault)
    }

    async fn fetch_relay(
        &self,
        origin: &EndpointId,
        manifest: &RelayManifest,
        range: Option<(u64, u64)>,
        dest: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<(), FilesFault> {
        let id: iroh::EndpointId = origin
            .0
            .parse()
            .map_err(|e| FilesFault::Io(format!("not an endpoint id: {} ({e})", origin.0)))?;

        // `(cumulative start, chunk)` for every chunk in order — the
        // manifest's own math for turning an absolute byte range into
        // which chunks it overlaps.
        let mut at = 0u64;
        let positioned: Vec<(u64, &RelayChunk)> = manifest
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

        // Bounded regardless of how large a "chunk" is: the whole-tier
        // case is one chunk for the entire file, so without this a
        // single-chunk multi-gigabyte take would hold itself whole in
        // memory on its way through — precisely the failure this relay
        // exists to avoid.
        const WINDOW: u64 = 4 << 20;

        let connection = self.blobs_connection(id).await?;
        let scratch = self.scratch_store().await?;
        use tokio::io::AsyncWriteExt as _;
        for (chunk_start, chunk) in positioned {
            let chunk_end = chunk_start + chunk.len;
            if chunk_end <= first || chunk_start > last {
                continue;
            }
            let hash = iroh_blobs::Hash::from(
                blake3::Hash::from_hex(&chunk.hash)
                    .map_err(|e| FilesFault::Io(format!("{}: not a hash: {e}", chunk.hash)))?
                    .as_bytes()
                    .to_owned(),
            );
            // The overlap with the caller's window, relative to this
            // chunk's own start.
            let want_start = first.max(chunk_start) - chunk_start;
            let want_end = last.min(chunk_end.saturating_sub(1)) - chunk_start;
            let mut cursor = want_start;
            while cursor <= want_end {
                let window_end = (cursor + WINDOW - 1).min(want_end);
                // Received into the on-disk `scratch` store, then read
                // back out and forgotten from `dest`'s point of view
                // once written — which is what keeps a multi-gigabyte
                // fetch bounded to one window of *this process's own
                // memory* rather than growing for the whole call. The
                // bytes on disk persist (see the field doc on `scratch`
                // for why that is the point, not a leak).
                let ranges: iroh_blobs::protocol::ChunkRanges =
                    iroh_blobs::protocol::ChunkRangesExt::bytes(cursor..window_end + 1);
                let request = iroh_blobs::protocol::GetRequest::blob_ranges(hash, ranges.clone());
                scratch
                    .remote()
                    .execute_get(connection.clone(), request)
                    .complete()
                    .await
                    .map_err(|e| {
                        FilesFault::Io(format!("{origin}: fetching {}: {e}", chunk.hash))
                    })?;
                let bytes = scratch
                    .blobs()
                    .export_bao(hash, ranges)
                    .data_to_vec()
                    .await
                    .map_err(|e| {
                        FilesFault::Io(format!("{origin}: reading fetched {}: {e}", chunk.hash))
                    })?;
                // `ChunkRangesExt::bytes` rounded [cursor, window_end]
                // up to whole BLAKE3 chunks to fetch something provable
                // — undo that here, or a caller asking for exactly 100
                // bytes gets back up to 2047 extra ones either side.
                let bytes = crate::lane::federation::trim_to_byte_range(bytes, cursor, window_end);
                dest.write_all(&bytes)
                    .await
                    .map_err(|e| FilesFault::Io(format!("relay write: {e}")))?;
                cursor = window_end + 1;
            }
        }
        Ok(())
    }
}
