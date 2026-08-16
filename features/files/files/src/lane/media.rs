//! `MediaService` — the byte lane, renditions and editor handoff.
//!
//! `files.scale.large-media` is the requirement this lane exists to
//! satisfy, and the whole design follows from one sentence in it: bytes
//! are never held whole in memory. Nothing on this surface returns
//! content inline. A read mints a [`ByteTicket`]; the ticket is redeemed
//! on a transport that streams.
//!
//! ## Why a ticket rather than a `Vec<u8>`
//!
//! An RPC that returns bytes has to materialise them, so a 244 GB
//! project becomes an allocation failure. A ticket moves the transfer
//! off the RPC surface entirely, which is also what keeps range
//! requests, resumption and (later) peer-to-peer delivery available
//! without any of them appearing in the trait.
//!
//! ## What a ticket is bound to
//!
//! A ticket names an **immutable** object, never a path:
//! [`FilesBackend::resolve_source`] pins a path to its content-addressed
//! identity once, and the ticket carries that address. A checkpoint
//! landing between minting and redemption therefore cannot change the
//! bytes under a half-served response — the `Content-Length` the route
//! advertised stays true. This is the same discipline the share-download
//! path already uses, for the same reason.
//!
//! A ticket also carries a **window** into that object. Redemption
//! offsets are relative to the window, so the redeemer sees an object of
//! exactly `length` bytes and can never address outside it. Today every
//! ticket's window is the whole object; the field exists because
//! `files.index.regions` will want a ticket for 0:40–0:52, and a window
//! bolted on later would be a second addressing scheme.
//!
//! ## Redemption
//!
//! [`FilesBackend::byte_ticket`] resolves a token to its ticket (for
//! response headers) and [`FilesBackend::redeem_bytes`] streams it into
//! an `AsyncWrite`, honouring a range. The signature is modelled on
//! `read_rendition_range` deliberately: taking a writer rather than
//! returning a buffer is what bounds memory to one chunk, and the HTTP
//! route pairs it with `tokio::io::duplex` + `ReaderStream` exactly as
//! `rendition_stream_response` already does.
//!
//! A ticket is **not** single-use. "Single-purpose" means bound to one
//! object and one window; a `<video>` element scrubbing a proxy issues
//! dozens of range requests against one grant, and expiring on first
//! redemption would break seeking outright.
//!
//! ## Why the ticket book is per-org and on disk
//!
//! [`crate::durable::Scoped`] rather than a module-level static, because
//! a static is shared by every org in the process: a token minted for
//! one org would redeem against another's backend. Tickets are
//! short-lived enough that durability is not the point — surviving a
//! restart is a harmless side effect, since expiry is re-checked at
//! redemption and expired rows are pruned on every mint.
//!
//! ## What is honestly missing
//!
//! - **Archive tickets.** `WriteService::archive` wants a ticket whose
//!   `length` is `None` because the stream is generated as it is sent.
//!   Nothing here can generate that stream — there is no archive writer
//!   in this crate — and minting a token that fails at redemption is
//!   worse than refusing, so no archive ticket is minted.
//! - **Ingress.** This lane moves bytes *out*. `UploadService::complete`
//!   needs bytes to arrive, which is a different transport.
//! - **Handoff collection over the wire.** [`MediaService::handoff`]
//!   mints and stores a handoff and
//!   [`FilesBackend::collect_handoff`] redeems it in-process, so the
//!   token is genuinely redeemable — but no editor-side integration in
//!   this crate calls it yet.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use files_proto::error::FilesFault;
use files_proto::id::{ContentId, RootId, VersionId};
use files_proto::model::{RenditionInfo, RenditionKind};
use files_proto::path::RootPath;
use files_proto::service::federation::{ByteRange, EndpointId};
use files_proto::service::legacy::{FilesError, FilesService};
use files_proto::service::media::{ByteFrame, ByteRequest, ByteTicket, Handoff, HandoffItem, HandoffTarget, MediaService};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::FilesBackend;
use crate::durable::Scoped;

/// How long a byte ticket stays redeemable.
///
/// Long enough that a client can mint, negotiate and start streaming
/// without racing; short enough that a leaked token is worthless by the
/// time it is found. A long-running download is unaffected — expiry is
/// checked when a range request arrives, not while one is in flight.
const TICKET_TTL_SECS: i64 = 10 * 60;

/// How long a handoff waits for its editor.
///
/// Longer than a byte ticket because the collector is a human moving
/// between two applications, not a program that already has the token.
const HANDOFF_TTL_SECS: i64 = 60 * 60;

/// Every rendition kind, for the ladder probe in
/// [`MediaService::renditions`].
const ALL_RENDITIONS: [RenditionKind; 5] = [
    RenditionKind::Proxy1080,
    RenditionKind::Proxy720,
    RenditionKind::Audio,
    RenditionKind::Peaks,
    RenditionKind::Filmstrip,
];

/// What a ticket points at.
///
/// Both variants are content addresses, not paths — see the module doc
/// on why a ticket must be immutable. They are separate variants because
/// they live in separate stores: renditions sit in a root's *private*
/// rendition CAS, which the source read paths deliberately cannot see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum ByteSource {
    /// Source content in the root's chunk CAS, by hex `FileId`.
    Source { root_id: Uuid, file_id: String },
    /// A derived rendition in the root's private rendition CAS.
    Rendition { root_id: Uuid, file_id: String },
    /// A tar generated as it is sent, over a selection of live-tree
    /// paths.
    ///
    /// The one source that is not an immutable object. Every other grant
    /// pins content at mint so a checkpoint landing mid-response cannot
    /// change bytes under an advertised length; an archive has no
    /// advertised length to break — `length: None`, `seekable: false` —
    /// and is a snapshot of the tree at redemption rather than at mint.
    /// That is a weaker promise, made explicitly rather than by
    /// accident.
    Archive { root_id: Uuid, paths: Vec<String> },
    /// An object on another server, pulled through as it is read.
    ///
    /// `files.peering.serving`: a host without the content still answers
    /// `read`, fetching from a host that has it. The token here is the
    /// *origin's*, never handed to our caller — they hold an ordinary
    /// local ticket, which is what keeps a federated file first-class
    /// rather than a redirect.
    ///
    /// Immutable like the others: the origin pinned a content address
    /// when it minted, so what this relays cannot change underneath a
    /// half-served response either.
    Relay {
        origin: EndpointId,
        /// Our authority at the origin, presented on every chunk — which
        /// is what makes a revocation land mid-transfer.
        secret: String,
        token: String,
    },
}

/// A minted grant. The wire [`ByteTicket`] is a projection of this.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Grant {
    source: ByteSource,
    /// Where the readable window starts in the underlying object.
    offset: u64,
    /// How much of it the holder may read. This is the ticket's whole
    /// world: redemption offsets are relative to `offset` and clamped
    /// to this, so a token cannot be walked outside its grant.
    length: u64,
    content_type: String,
    expires_at: DateTime<Utc>,
}

impl Grant {
    fn ticket(&self, token: String) -> ByteTicket {
        let generated = matches!(self.source, ByteSource::Archive { .. });
        ByteTicket {
            token,
            // Known for a stored object, because its length was read from
            // the store. `None` for a generated stream: an archive's size
            // is not known until it has been produced, and guessing it
            // would put a number on the wire that the body then fails to
            // match.
            length: (!generated).then_some(self.length),
            // Honest, not aspirational: both stores read by range, and a
            // stream generated in one pass cannot.
            seekable: !generated,
            content_type: self.content_type.clone(),
            expires_at: self.expires_at,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TicketBook(HashMap<String, Grant>);

#[derive(Debug, Default, Serialize, Deserialize)]
struct HandoffBook(HashMap<String, Handoff>);

/// Per-org, so a token minted against one org's backend cannot be
/// redeemed against another's.
static TICKETS: Scoped<TicketBook> = Scoped::new("byte-tickets");
static HANDOFFS: Scoped<HandoffBook> = Scoped::new("handoffs");

/// A 256-bit token, from two v4 UUIDs.
///
/// A capability is only as good as its unguessability, and a single
/// UUID's 122 bits is thin for something that stands in for a file's
/// contents. Two of them rather than a new `rand` dependency — this
/// crate already has a CSPRNG through `uuid`'s v4 generator.
fn mint_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// The legacy error type onto the v2 fault type.
///
/// `NotFound` becomes `Invalid` rather than a typed absence: the legacy
/// variant carries prose, so which of root, path or version was missing
/// is not recoverable from it. Callers that need the typed answer get it
/// from the `root_or_fault` / `validate` checks that run first.
fn fault(err: FilesError) -> FilesFault {
    match err {
        FilesError::NotFound(m) | FilesError::BadRequest(m) => FilesFault::Invalid(m),
        FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
        FilesError::Io(m) => FilesFault::Io(m),
    }
}

/// A content type from the file's extension.
///
/// From the name, never from sniffing the bytes — the same rule the
/// rendition route follows. Sniffing means reading content to answer a
/// metadata question, and a wrong guess on a media file is the
/// difference between a browser playing it and downloading it.
fn content_type_for(path: &RootPath) -> String {
    let ext = path
        .name()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "md" | "txt" => "text/plain; charset=utf-8",
        // The honest default. A guess dressed up as a specific type is
        // worse than admitting the extension says nothing.
        _ => "application/octet-stream",
    }
    .to_string()
}

/// A path that can name a file's bytes.
fn readable(path: &RootPath) -> Result<RootPath, FilesFault> {
    let path = path.validate()?;
    if path.is_root() {
        return Err(FilesFault::invalid("the root itself has no bytes to read"));
    }
    Ok(path)
}

/// The byte lane's own half: minting, inspecting and redeeming tickets,
/// plus handoff collection.
impl FilesBackend {
    /// Record a grant and project it onto the wire ticket.
    ///
    /// Expired rows are pruned here rather than on a timer: minting is
    /// the only operation that grows the book, so it is the only place
    /// that has to bound it, and a sweep that runs exactly as often as
    /// the book grows needs no scheduler.
    fn mint(&self, grant: Grant) -> ByteTicket {
        let token = mint_token();
        let now = Utc::now();
        TICKETS.write(self, |book| {
            book.0.retain(|_, g| g.expires_at > now);
            book.0.insert(token.clone(), grant.clone());
        });
        grant.ticket(token)
    }

    /// The ticket a token stands for — what an HTTP route needs to build
    /// `Content-Type`, `Content-Length` and `Accept-Ranges` *before* it
    /// starts streaming.
    ///
    /// An unknown token and an expired one answer identically, on
    /// purpose: the difference tells a prober whether a token was ever
    /// real.
    pub fn byte_ticket(&self, token: &str) -> Result<ByteTicket, FilesFault> {
        let now = Utc::now();
        TICKETS
            .read(self, |book| book.0.get(token).cloned())
            .filter(|grant| grant.expires_at > now)
            .map(|grant| grant.ticket(token.to_string()))
            .ok_or_else(|| FilesFault::invalid("no such byte ticket, or it has expired"))
    }

    /// Stream a ticket's bytes into `dest`, honouring a range.
    ///
    /// `range` is an inclusive `(first, last)` byte pair **relative to
    /// the ticket**, matching HTTP's `Range` semantics — the caller
    /// parses the header against `ByteTicket::length` and passes the
    /// result through unchanged. `None` streams the whole ticket.
    ///
    /// Takes an `AsyncWrite` rather than returning bytes so memory stays
    /// bounded to one chunk however large the object is; both stores
    /// read only the chunks the window overlaps, which is what makes
    /// seeking the middle of a multi-gigabyte file cheap.
    ///
    /// A mid-stream failure can only truncate the body — by the time
    /// this runs the route has already committed its status line — so
    /// the client sees a short read against the advertised length rather
    /// than a 500.
    // t[impl files.scale.large-media] — range-read, one chunk at a time
    pub async fn redeem_bytes<W>(
        &self,
        token: &str,
        range: Option<(u64, u64)>,
        dest: &mut W,
    ) -> Result<(), FilesFault>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let now = Utc::now();
        let grant = TICKETS
            .read(self, |book| book.0.get(token).cloned())
            .filter(|grant| grant.expires_at > now)
            .ok_or_else(|| FilesFault::invalid("no such byte ticket, or it has expired"))?;

        let (start, len) = match range {
            Some((first, last)) => {
                if first > last || last >= grant.length {
                    return Err(FilesFault::invalid(format!(
                        "range {first}-{last} lies outside this ticket's {} bytes",
                        grant.length
                    )));
                }
                (first, last - first + 1)
            }
            None => (0, grant.length),
        };
        // Relative to the grant's window, so a token can never be walked
        // past what it was minted for.
        let start = grant.offset + start;

        match &grant.source {
            ByteSource::Source { root_id, file_id } => {
                let chunks = self
                    .with_version_store(*root_id, |vs| vs.chunks().clone())
                    .map_err(fault)?;
                let fid = files_store::chunk::FileId::from_hex(file_id)
                    .map_err(|e| FilesFault::Store(format!("{file_id}: {e}")))?;
                chunks
                    .read_range(fid, start, len, dest)
                    .await
                    .map_err(|e| FilesFault::Io(format!("source {file_id}: {e}")))
            }
            ByteSource::Rendition { root_id, file_id } => self
                .read_rendition_range(*root_id, file_id, start, len, dest)
                .await
                .map_err(fault),
            ByteSource::Relay {
                origin,
                secret,
                token,
            } => {
                let Some(port) = self.remote_files() else {
                    return Err(FilesFault::Unavailable {
                        path: files_proto::path::RootPath::root(),
                    });
                };
                // Chunk at a time, so relaying a 4 GB reel costs one
                // buffer here rather than 4 GB. This is the same bound
                // the origin enforces; doing it on both sides means
                // neither has to trust the other's arithmetic.
                use tokio::io::AsyncWriteExt as _;
                let mut sent = 0u64;
                while sent < len {
                    let want = u32::try_from((len - sent).min(u64::from(ByteRange::MAX_LEN)))
                        .unwrap_or(ByteRange::MAX_LEN);
                    let chunk = port
                        .fetch_offered(origin, secret, token, ByteRange::new(start + sent, want))
                        .await?;
                    if chunk.is_empty() {
                        // The origin ran out early. Truncating is the
                        // only signal left once bytes are on the wire,
                        // so say so rather than pad.
                        return Err(FilesFault::Io(format!(
                            "{origin}: relay ended {} bytes short",
                            len - sent
                        )));
                    }
                    sent += chunk.len() as u64;
                    dest.write_all(&chunk)
                        .await
                        .map_err(|e| FilesFault::Io(format!("relay write: {e}")))?;
                }
                Ok(())
            }
            ByteSource::Archive { root_id, paths } => {
                if range.is_some() {
                    // The ticket said `seekable: false`. Refusing is the
                    // only honest answer: satisfying a range would mean
                    // generating everything before it and discarding it,
                    // which is the cost the caller was told to avoid.
                    return Err(FilesFault::invalid(
                        "an archive is generated in one pass and cannot be ranged",
                    ));
                }
                self.write_archive(*root_id, paths, dest).await
            }
        }
    }


    /// Generate a tar over a selection, straight into `dest`.
    ///
    /// Nothing is materialised: each file is opened, streamed a buffer at
    /// a time into the entry, and closed. A selection of a whole root
    /// costs one buffer, not one archive.
    ///
    /// Entries are emitted depth-first in the order given, with each
    /// directory announced before its contents so an extractor never has
    /// to create a parent it was not told about.
    async fn write_archive<W>(
        &self,
        root_id: Uuid,
        paths: &[String],
        dest: &mut W,
    ) -> Result<(), FilesFault>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        use tokio::io::AsyncReadExt as _;

        let root = crate::lane::root_or_fault(self, RootId::new(root_id))?;
        let base = crate::lane::lane_tree(&root)?;
        let mut tar = crate::tarball::Tar::new(dest);

        // Flatten first so a failure to walk is reported before a byte of
        // archive is on the wire — after that, the only way to signal is
        // to truncate.
        let mut queue: Vec<String> = paths.to_vec();
        let mut entries: Vec<(String, bool, u64, u64)> = Vec::new();
        while let Some(rel) = queue.pop() {
            let disk = base.join(&rel);
            let Ok(meta) = std::fs::metadata(&disk) else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            if meta.is_dir() {
                entries.push((rel.clone(), true, 0, mtime));
                if let Ok(read) = std::fs::read_dir(&disk) {
                    for child in read.flatten() {
                        let name = child.file_name().to_string_lossy().into_owned();
                        if name == files_proto::consts::STORE_DIR
                            || name == files_proto::consts::MARKER_FILE
                        {
                            continue;
                        }
                        queue.push(format!("{rel}/{name}"));
                    }
                }
            } else {
                entries.push((rel.clone(), false, meta.len(), mtime));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut buf = vec![0u8; 64 * 1024];
        for (rel, is_dir, size, mtime) in entries {
            if is_dir {
                tar.directory(&rel, mtime)
                    .await
                    .map_err(|e| FilesFault::Io(e.to_string()))?;
                continue;
            }
            let mut file = tokio::fs::File::open(base.join(&rel))
                .await
                .map_err(FilesFault::io)?;
            let mut entry = tar
                .file(&rel, size, mtime)
                .await
                .map_err(|e| FilesFault::Io(e.to_string()))?;
            loop {
                let n = file.read(&mut buf).await.map_err(FilesFault::io)?;
                if n == 0 {
                    break;
                }
                entry
                    .write(&buf[..n])
                    .await
                    .map_err(|e| FilesFault::Io(e.to_string()))?;
            }
            entry
                .close()
                .await
                .map_err(|e| FilesFault::Io(e.to_string()))?;
        }
        tar.finish()
            .await
            .map_err(|e| FilesFault::Io(e.to_string()))
    }

    /// Mint a ticket for an archive of `paths`.
    ///
    /// Called by [`crate::lane::write`], which owns `archive` on the
    /// wire but has no ticket store of its own.
    /// Turn an origin's ticket into one of ours.
    ///
    /// Length and content type come from the origin — it read them from
    /// its own store — so our caller is told the truth about an object
    /// we do not hold. Expiry is ours and shorter is fine: re-minting
    /// costs one round trip and a stale relay grant is worth less than
    /// a fresh one.
    pub(crate) fn mint_relay_ticket(
        &self,
        origin: EndpointId,
        secret: String,
        remote: ByteTicket,
    ) -> ByteTicket {
        self.mint(Grant {
            source: ByteSource::Relay {
                origin,
                secret,
                token: remote.token,
            },
            offset: 0,
            // An origin that could not state a length minted an archive
            // or a generated stream; relaying one is not supported, and
            // a zero-length grant refuses every range rather than
            // half-serving.
            length: remote.length.unwrap_or(0),
            content_type: remote.content_type,
            expires_at: Utc::now() + Duration::hours(1),
        })
    }

    pub(crate) fn mint_archive(
        &self,
        root_id: RootId,
        paths: &[files_proto::path::RootPath],
    ) -> Result<ByteTicket, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let grant = Grant {
            source: ByteSource::Archive {
                root_id: root_id.get(),
                paths: paths.iter().map(|p| p.as_str().to_string()).collect(),
            },
            offset: 0,
            length: 0,
            content_type: "application/x-tar".to_string(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
        };
        Ok(self.mint(grant))
    }

    /// Collect a handoff by its token, which consumes it.
    ///
    /// Consumed rather than reusable because a handoff is a delivery: a
    /// bin arriving twice is a duplicated bin, and unlike a byte ticket
    /// there is no seeking to support.
    ///
    /// Nothing in this crate calls this yet — see the module doc. It
    /// exists so the token [`MediaService::handoff`] mints is genuinely
    /// redeemable rather than decorative.
    // t[impl files.handoff.editor] — the collection side of the handoff
    pub fn collect_handoff(&self, token: &str) -> Result<Handoff, FilesFault> {
        let now = Utc::now();
        HANDOFFS
            .write(self, |book| {
                book.0.retain(|_, h| h.expires_at > now);
                book.0.remove(token)
            })
            .ok_or_else(|| FilesFault::invalid("no such handoff, or it has expired"))
    }

    /// Pin a path's content and mint a ticket for it.
    ///
    /// `at` is `None` for the checkpoint head, or a commit prefix for a
    /// past version. Resolution happens once, here — see the module doc
    /// on why the ticket carries the address rather than the path.
    async fn source_ticket(
        &self,
        root_id: RootId,
        path: &RootPath,
        at: Option<String>,
    ) -> Result<ByteTicket, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = readable(path)?;
        let (length, file_id) = self
            .resolve_source(root_id.get(), path.as_str().to_string(), at)
            .await
            .map_err(fault)?;
        Ok(self.mint(Grant {
            source: ByteSource::Source {
                root_id: root_id.get(),
                file_id,
            },
            offset: 0,
            length,
            content_type: content_type_for(&path),
            expires_at: Utc::now() + Duration::seconds(TICKET_TTL_SECS),
        }))
    }
}

impl MediaService for FilesBackend {
    /// A ticket for a file's current content.
    ///
    /// "Current" means the checkpoint head, not the bytes on disk this
    /// instant. A file being written to right now has no stable length
    /// and no stable content, so a ticket for it could only promise
    /// something it cannot keep; the head is the most recent state the
    /// store can pin. A file never yet checkpointed is therefore not
    /// readable through this lane.
    // t[impl files.scale.large-media]
    async fn read(&self, root_id: RootId, path: RootPath) -> Result<ByteTicket, FilesFault> {
        // A root accepted from elsewhere has no live tree on this disk,
        // so resolving it locally would report a missing path for a file
        // that exists. The tree lane asks the same question before it
        // walks, for the same reason.
        if self.remote_of(root_id).is_some() {
            return self.read_remote(root_id, &path).await;
        }
        self.source_ticket(root_id, &path, None).await
    }

    /// A ticket for a file's content at a past version.
    // t[impl files.scale.large-media]
    async fn read_at(
        &self,
        root_id: RootId,
        path: RootPath,
        version: VersionId,
    ) -> Result<ByteTicket, FilesFault> {
        // `commit_prefix` is the canonical conversion back to something
        // the store's prefix resolver accepts — see `VersionId`.
        self.source_ticket(root_id, &path, Some(version.commit_prefix()))
            .await
    }

    /// A ticket for content by address, with no path context.
    ///
    /// The federation read path. A [`ContentId`] is the hash of a chunk
    /// manifest, so "do we already hold this?" is a store lookup — and
    /// that is the whole implementation. Bytes held locally resolve
    /// without reaching their origin server, because there is nothing
    /// here that knows what an origin server is.
    ///
    /// The scan is over this org's roots only (the registry is per-org)
    /// and stops at the first holder. Chunk stores are per-root rather
    /// than per-org, which is what makes it a scan at all; a shared
    /// per-org store would make it one lookup, and that is a storage
    /// change rather than a lane change.
    // t[impl files.scale.large-media] — content resolves without its origin
    async fn read_content(&self, content: ContentId) -> Result<ByteTicket, FilesFault> {
        let fid = files_store::chunk::FileId::from_hex(content.as_str())
            .map_err(|e| FilesFault::invalid(format!("{content}: not a content address: {e}")))?;

        for root in self.registry_list() {
            let Ok(chunks) = self.with_version_store(root.id, |vs| vs.chunks().clone()) else {
                // A software root's history is git's — it has no chunk
                // store, and that is not an error here.
                continue;
            };
            if !chunks.has(fid).await {
                continue;
            }
            let length = chunks
                .content_len(fid)
                .await
                .map_err(|e| FilesFault::Io(format!("{content}: {e}")))?;
            return Ok(self.mint(Grant {
                source: ByteSource::Source {
                    root_id: root.id,
                    file_id: content.to_string(),
                },
                offset: 0,
                length,
                // No path, so no extension, so nothing honest to say
                // about the format.
                content_type: "application/octet-stream".to_string(),
                expires_at: Utc::now() + Duration::seconds(TICKET_TTL_SECS),
            }));
        }
        Err(FilesFault::invalid(format!(
            "{content}: no root here holds that content"
        )))
    }

    /// Renditions already generated for a file.
    ///
    /// A query, so it must not generate: listing the ladder of a 4K
    /// master would otherwise mean transcoding it. Presence is read from
    /// the rendition store's on-disk index — one `exists` per kind, no
    /// store handle — and only the kinds that pass are asked for, which
    /// the backend then answers from its cache.
    ///
    /// Reading the index filename convention rather than calling the
    /// store is a coupling worth naming: the backend's rendition store
    /// handle is private (a second `FsStore` on one directory hangs, so
    /// this lane must not open its own) and exposes no presence query.
    /// `renditions_lists_what_was_generated` in the integration tests
    /// fails if the convention drifts.
    async fn renditions(
        &self,
        root_id: RootId,
        path: RootPath,
    ) -> Result<Vec<RenditionInfo>, FilesFault> {
        let root = crate::lane::root_or_fault(self, root_id)?;
        let path = readable(&path)?;
        let (_len, source) = self
            .resolve_source(root_id.get(), path.as_str().to_string(), None)
            .await
            .map_err(fault)?;
        let index = crate::transcode::rendition_dir(crate::lane::lane_tree(&root)?).join("index");

        let mut out = Vec::new();
        for kind in ALL_RENDITIONS {
            let record = index.join(format!(
                "{source}.{}.{}.json",
                files_transcode::RECIPE_VERSION,
                kind.tag()
            ));
            if !record.exists() {
                continue;
            }
            match FilesService::rendition(self, root_id.get(), path.as_str().to_string(), kind).await
            {
                Ok(info) => out.push(info),
                // Indexed but unreadable: its content was swept between
                // the probe and the read. Reporting the rest is better
                // than failing a listing over a cache entry.
                Err(err) => {
                    tracing::debug!(%root_id, %path, ?kind, %err, "files: rendition listed but not readable");
                }
            }
        }
        Ok(out)
    }

    /// A ticket for one rendition, requesting it if absent.
    ///
    /// Delegates the generate-once-and-cache decision to the backend's
    /// transcode pipeline and only mints over the result — this lane
    /// owns the ticket, not the ladder.
    // t[impl files.scale.large-media]
    async fn rendition(
        &self,
        root_id: RootId,
        path: RootPath,
        kind: RenditionKind,
    ) -> Result<ByteTicket, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = readable(&path)?;
        let info =
            FilesService::rendition(self, root_id.get(), path.as_str().to_string(), kind)
                .await
                .map_err(fault)?;
        Ok(self.mint(Grant {
            source: ByteSource::Rendition {
                root_id: root_id.get(),
                file_id: info.file_id,
            },
            offset: 0,
            length: info.len,
            // The rendition's own declared type, never the source's:
            // a proxy of a `.mov` is an `mp4`.
            content_type: info.mime,
            expires_at: Utc::now() + Duration::seconds(TICKET_TTL_SECS),
        }))
    }

    /// Hand a selection to an editor as a bin or a timeline.
    ///
    /// Content stays in place: an item is a `(root, path, region)`
    /// triple, and nothing is copied or transcoded to build one. The
    /// region travels verbatim — a hit covering 0:40–0:52 is stored and
    /// collected as that range, never widened to the whole clip — which
    /// is the substance of `files.handoff.editor`.
    ///
    /// Every item is checked to exist before the handoff is minted, so a
    /// collector never receives a bin half of whose clips are missing.
    /// Nothing in this crate collects one yet; see the module doc.
    ///
    /// `HandoffItem::region` is `files_proto`'s one
    /// [`Region`](files_proto::service::media::Region) — the same scheme
    /// search hits and review annotations address with. A private
    /// handoff-shaped region type would be the exact failure
    /// `files.index.regions` names.
    // t[impl files.handoff.editor]
    // t[impl files.index.regions] — one region scheme, three consumers
    async fn handoff(
        &self,
        name: String,
        target: HandoffTarget,
        items: Vec<HandoffItem>,
    ) -> Result<Handoff, FilesFault> {
        if items.is_empty() {
            return Err(FilesFault::invalid("a handoff with no items delivers nothing"));
        }
        let mut checked = Vec::with_capacity(items.len());
        for item in items {
            let root = crate::lane::root_or_fault(self, item.root_id)?;
            let path = readable(&item.path)?;
            let (disk, _) = self.resolve_root_file(&root, path.as_str())?;
            if disk.symlink_metadata().is_err() {
                return Err(FilesFault::PathNotFound(path));
            }
            checked.push(HandoffItem {
                root_id: item.root_id,
                path,
                // Verbatim. Normalising a region here would be the
                // "arrives as the whole clip" failure the requirement
                // names.
                region: item.region,
            });
        }

        let handoff = Handoff {
            name,
            target,
            items: checked,
            token: mint_token(),
            expires_at: Utc::now() + Duration::seconds(HANDOFF_TTL_SECS),
        };
        let now = Utc::now();
        HANDOFFS.write(self, |book| {
            book.0.retain(|_, h| h.expires_at > now);
            book.0.insert(handoff.token.clone(), handoff.clone());
        });
        Ok(handoff)
    }
}

// ── The byte lane over vox ────────────────────────────────────────────

/// Bytes per frame.
///
/// vox carries a fixed per-channel credit (16 frames by default), so this
/// size times that credit is what may be in flight before the sink makes
/// the producer wait — a megabyte, which is enough to keep a link busy
/// and small enough that a stalled reader costs almost nothing.
const FRAME: usize = 64 * 1024;

impl files_proto::service::media::MediaServiceStreamSource for FilesBackend {
    // t[impl files.scale.large-media] — streamed, ranged, never held whole
    // t[impl files.scale.transport] — bytes ride vox, with vox's flow control
    fn bytes_attach(&self, request: ByteRequest, sink: architect::vox::Tx<ByteFrame>) {
        let backend = self.clone();
        tokio::spawn(async move {
            stream_bytes(&backend, &request, &sink).await;
        });
    }
}

/// Drive one byte stream to its end, or to an honest failure.
///
/// Every send `await`s, which is the point: vox's mailbox blocks the
/// producer once the credit is spent, so a client that stops reading
/// stops us reading too. Nothing here holds more than one frame, whatever
/// the file's size.
async fn stream_bytes(
    backend: &FilesBackend,
    request: &ByteRequest,
    sink: &architect::vox::Tx<ByteFrame>,
) {
    let ticket = match backend.byte_ticket(&request.token) {
        Ok(t) => t,
        Err(fault) => {
            let _ = sink.send(ByteFrame::Failed(fault)).await;
            return;
        }
    };

    let total = ticket.length.unwrap_or(0);
    let range = request.range.map(|r| (r.first, r.last));
    let length = match range {
        Some((first, last)) if last >= first && last < total => last - first + 1,
        // A range past the end is the caller's error, not a truncation to
        // report mid-stream — say so before any bytes are promised.
        Some(_) => {
            let _ = sink
                .send(ByteFrame::Failed(FilesFault::invalid(
                    "byte range lies outside the ticket",
                )))
                .await;
            return;
        }
        None => total,
    };

    if sink
        .send(ByteFrame::Opened {
            length,
            total,
            content_type: ticket.content_type.clone(),
        })
        .await
        .is_err()
    {
        return;
    }

    // A duplex pipe rather than a Vec: `redeem_bytes` takes an
    // `AsyncWrite` precisely so memory stays bounded to a frame, and
    // collecting first would defeat the whole lane.
    let (mut writer, mut reader) = tokio::io::duplex(FRAME);
    let reading = {
        let backend = backend.clone();
        let token = request.token.clone();
        tokio::spawn(async move { backend.redeem_bytes(&token, range, &mut writer).await })
    };

    use tokio::io::AsyncReadExt as _;
    let mut buf = vec![0u8; FRAME];
    let mut offset = 0u64;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if sink
                    .send(ByteFrame::Chunk {
                        offset,
                        bytes: buf[..n].to_vec(),
                    })
                    .await
                    .is_err()
                {
                    // The subscriber went away. Dropping the reader ends
                    // the producing task with it.
                    return;
                }
                offset += n as u64;
            }
            Err(err) => {
                let _ = sink.send(ByteFrame::Failed(FilesFault::io(err))).await;
                return;
            }
        }
    }

    // The reader saw EOF, which a failed producer also causes — so the
    // producer's own result decides whether this was `Done`. Without
    // this, a mid-stream failure would arrive as a short but successful
    // stream, which is the exact ambiguity the HTTP fallback cannot
    // escape and this lane exists to escape.
    let outcome = match reading.await {
        Ok(Ok(())) => ByteFrame::Done,
        Ok(Err(fault)) => ByteFrame::Failed(fault),
        Err(join) => ByteFrame::Failed(FilesFault::Internal(join.to_string())),
    };
    let _ = sink.send(outcome).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RootPath {
        RootPath::parse(s).expect("test path")
    }

    #[test]
    fn a_content_type_comes_from_the_extension_and_never_from_a_guess() {
        assert_eq!(content_type_for(&p("takes/cut.mov")), "video/quicktime");
        assert_eq!(content_type_for(&p("mix.WAV")), "audio/wav");
        assert_eq!(
            content_type_for(&p("session")),
            "application/octet-stream",
            "no extension says nothing, and saying nothing is honest"
        );
    }

    #[test]
    fn the_root_itself_has_no_bytes() {
        assert!(matches!(
            readable(&RootPath::root()),
            Err(FilesFault::Invalid(_))
        ));
        // `RootPath` is `#[serde(transparent)]`, so a hostile peer's
        // path never saw `parse` — the guard has to run on this side.
        let hostile: RootPath =
            serde_json::from_str("\"../../etc/passwd\"").expect("transparent newtype");
        assert!(matches!(readable(&hostile), Err(FilesFault::BadPath(_))));
    }

    #[test]
    fn a_token_is_wide_enough_to_be_a_capability() {
        let a = mint_token();
        let b = mint_token();
        assert_eq!(a.len(), 64, "256 bits, hex");
        assert_ne!(a, b);
    }
}
