//! **The Files lanes as an app's store.**
//!
//! Keyflow keeping charts, Session keeping takes and DAW sessions, Signal
//! keeping impulse responses: each wants the same five things from Task,
//! and none of them should have to learn the lanes' streaming halves to
//! get them.
//!
//! | want | call |
//! |---|---|
//! | somewhere to keep things | [`FilesClient::ensure_root`] |
//! | save bytes, safely | [`FilesClient::put`] / [`FilesClient::put_reader`] with [`Save`] |
//! | read them back | [`FilesClient::get`] / [`FilesClient::read_range`] / [`FilesClient::read_to`] |
//! | follow a manifest's reference | [`FilesClient::resolve`] / [`FilesClient::get_ref`] |
//! | hear what changed | [`FilesClient::events`] |
//!
//! ## Transport
//!
//! Nothing here opens a connection. [`FilesClient`] holds the generated
//! typed clients, and those carry no transport assumption — so the same
//! code runs in a browser over `task_dial::establish_at`, natively over
//! `task_client`, and in a test over an `architect::LocalServer`. Futures
//! are driven with `futures::join!`, never a runtime's `spawn`, which is
//! what keeps it wasm-clean.
//!
//! ```ignore
//! let url = "wss://task.example.com/org/acme/vox";
//! let files = FilesClient::new(
//!     task_dial::establish_at(url, Some(&token)).await?,
//!     task_dial::establish_at(url, Some(&token)).await?,
//!     task_dial::establish_at(url, Some(&token)).await?,
//!     task_dial::establish_at(url, Some(&token)).await?,
//!     task_dial::establish_at(url, Some(&token)).await?,
//! );
//! let irs = files.ensure_root("signal/impulse-responses", "Impulse responses", RootFlavor::Media).await?;
//! let saved = files.put(root_id(&irs), "Cab/4x12 V30.wav", &wav, Save::create_only()).await?;
//! // Pin the manifest to exactly these bytes:
//! let pin = saved.content;
//! ```
//!
//! ## Saving safely
//!
//! A [`Save`] carries the conflict policy and, optionally, the etag the
//! caller last saw. Two machines saving the same session file cannot then
//! overwrite each other silently: the second gets
//! [`FilesFault::Stale`](files_proto::FilesFault::Stale) and decides. The
//! etag to pass is the `content` of the [`CatalogueEntry`] the previous
//! save (or [`FilesClient::entry`]) returned.

use architect::vox;
use files_proto::error::FilesFault;
use files_proto::id::{ContentId, RootId, UploadId};
use files_proto::model::{FileRootInfo, RootFlavor};
use files_proto::path::RootPath;
use files_proto::service::FilesEvent;
use files_proto::service::media::{ByteFrame, ByteRange, ByteRequest, ByteTicket};
use files_proto::service::roots::CreateRequest;
use files_proto::service::tree::CatalogueEntry;
use files_proto::service::upload::{ChunkRange, Expect, UploadFrame, UploadSpec};
use files_proto::service::write::OnConflict;
use files_proto::{
    MediaServiceClient, MediaServiceStreamClient, RootsServiceClient, TreeServiceClient,
    TreeServiceStreamClient, UploadServiceClient,
};

/// Bytes per upload frame. Small enough that vox's credit paces a slow
/// server without a large buffer in flight, large enough that framing is
/// not the cost.
pub const FRAME: usize = 256 * 1024;

/// How many times [`FilesClient::put`] re-sends what the server still
/// reports missing before giving up. A dropped stream is resumed from
/// the server's own account of what arrived, never restarted.
pub const RESEND_ATTEMPTS: usize = 3;

/// What can go wrong, split the way a caller acts on it.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server refused, with the domain's reason — `Stale`, `Exists`,
    /// `Denied`, `PathNotFound`. Act on these.
    #[error("{0}")]
    Fault(FilesFault),
    /// The call did not complete: connection, encoding, a denial at the
    /// gate. Retry or reconnect.
    #[error("transport: {0}")]
    Transport(String),
    /// A byte stream stopped short or said something out of order.
    #[error("stream: {0}")]
    Stream(String),
}

impl ClientError {
    /// The domain fault, when it was one.
    #[must_use]
    pub fn fault(&self) -> Option<&FilesFault> {
        match self {
            Self::Fault(f) => Some(f),
            _ => None,
        }
    }

    /// Whether this is a safe save refused because the file moved on.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Fault(FilesFault::Stale { .. }))
    }
}

impl From<vox::VoxError<FilesFault>> for ClientError {
    fn from(e: vox::VoxError<FilesFault>) -> Self {
        match e {
            vox::VoxError::User(fault) => Self::Fault(*fault),
            other => Self::Transport(format!("{other:?}")),
        }
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// How a save treats what is already there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    pub on_conflict: OnConflict,
    pub expect: Option<Expect>,
}

impl Save {
    /// Only if nothing is there. Never displaces anything.
    #[must_use]
    pub fn create_only() -> Self {
        Self {
            on_conflict: OnConflict::Fail,
            expect: Some(Expect::Absent),
        }
    }

    /// Replace exactly the content last seen as `etag`, and nothing newer.
    /// The save an editor makes.
    #[must_use]
    pub fn replacing(etag: ContentId) -> Self {
        Self {
            on_conflict: OnConflict::Replace,
            expect: Some(Expect::Content(etag)),
        }
    }

    /// Replace whatever is there. Recorded as a new version; the old one
    /// stays in history. Last writer wins — use [`Self::replacing`] when
    /// two machines might save the same file.
    #[must_use]
    pub fn overwrite() -> Self {
        Self {
            on_conflict: OnConflict::Replace,
            expect: None,
        }
    }

    /// Land beside an occupant under a derived name, never on it.
    #[must_use]
    pub fn keep_both() -> Self {
        Self {
            on_conflict: OnConflict::KeepBoth,
            expect: None,
        }
    }
}

/// What a content reference resolved to.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// The path holds the pinned bytes (or the reference pinned nothing).
    Current(CatalogueEntry),
    /// The path holds something else now; the pinned bytes may still be
    /// readable by address with [`FilesClient::get_ref`].
    Moved {
        now: CatalogueEntry,
        pinned: ContentId,
    },
    /// Nothing at the path. A pinned reference may still be readable by
    /// address.
    Missing { pinned: Option<ContentId> },
}

/// The typed clients one app needs, held together.
#[derive(Clone)]
pub struct FilesClient {
    pub roots: RootsServiceClient,
    pub tree: TreeServiceClient,
    pub uploads: UploadServiceClient,
    pub media: MediaServiceClient,
    pub bytes: MediaServiceStreamClient,
}

impl FilesClient {
    #[must_use]
    pub fn new(
        roots: RootsServiceClient,
        tree: TreeServiceClient,
        uploads: UploadServiceClient,
        media: MediaServiceClient,
        bytes: MediaServiceStreamClient,
    ) -> Self {
        Self {
            roots,
            tree,
            uploads,
            media,
            bytes,
        }
    }

    // ── Roots ──────────────────────────────────────────────────────

    /// The app's root at `dir` (relative to the org's files area), made
    /// if it does not exist yet. Safe to call on every start.
    pub async fn ensure_root(
        &self,
        dir: &str,
        name: &str,
        flavor: RootFlavor,
    ) -> Result<FileRootInfo> {
        Ok(self
            .roots
            .create(CreateRequest {
                dir: dir.to_string(),
                name: name.to_string(),
                flavor,
            })
            .await?)
    }

    /// One entry's catalogue record — its size and etag — without
    /// listing its parent.
    pub async fn entry(&self, root: RootId, path: &str) -> Result<CatalogueEntry> {
        Ok(self.tree.entry(root, parse(path)?).await?)
    }

    // ── Saving ─────────────────────────────────────────────────────

    /// Save `bytes` at `path`. Returns the landed entry; its `content` is
    /// the etag to pass as [`Save::replacing`] next time.
    ///
    /// Content the server already holds transfers nothing. A stream that
    /// drops part-way is resumed from what the server reports missing,
    /// up to [`RESEND_ATTEMPTS`] times.
    pub async fn put(
        &self,
        root: RootId,
        path: &str,
        bytes: &[u8],
        save: Save,
    ) -> Result<CatalogueEntry> {
        let plan = self
            .uploads
            .begin(UploadSpec {
                root_id: root,
                path: parse(path)?,
                size: bytes.len() as u64,
                content: None,
                modified_at: None,
                expect: save.expect.clone(),
            })
            .await?;
        let mut needed = plan.needed;
        for _ in 0..=RESEND_ATTEMPTS {
            if needed.is_empty() {
                break;
            }
            needed = self.send_ranges(plan.upload_id, bytes, &needed).await?;
        }
        if !needed.is_empty() {
            return Err(ClientError::Stream(format!(
                "{path}: the server still reports {} range(s) missing after {RESEND_ATTEMPTS} resends",
                needed.len()
            )));
        }
        Ok(self
            .uploads
            .complete(plan.upload_id, save.on_conflict)
            .await?)
    }

    /// Save `size` bytes read from `reader` at `path`, without holding the
    /// whole file — the shape for a multi-gigabyte take. Frames are read
    /// and sent one at a time; nothing is resent, so a dropped stream
    /// fails and the caller begins again (content the server already
    /// received is not transferred twice when it lands in a media root).
    pub async fn put_reader<R>(
        &self,
        root: RootId,
        path: &str,
        size: u64,
        mut reader: R,
        save: Save,
    ) -> Result<CatalogueEntry>
    where
        R: futures::io::AsyncRead + Unpin,
    {
        use futures::io::AsyncReadExt as _;

        let plan = self
            .uploads
            .begin(UploadSpec {
                root_id: root,
                path: parse(path)?,
                size,
                content: None,
                modified_at: None,
                expect: save.expect.clone(),
            })
            .await?;
        if !plan.needed.is_empty() {
            let (tx, rx) = vox::channel::<UploadFrame>();
            let sending = async move {
                let mut offset = 0u64;
                let mut buf = vec![0u8; FRAME];
                while offset < size {
                    let want = usize::try_from((size - offset).min(FRAME as u64)).unwrap_or(FRAME);
                    reader
                        .read_exact(&mut buf[..want])
                        .await
                        .map_err(|e| ClientError::Stream(format!("reading the source: {e}")))?;
                    tx.send(UploadFrame::Chunk {
                        offset,
                        bytes: buf[..want].to_vec(),
                    })
                    .await
                    .map_err(|e| ClientError::Transport(format!("{e:?}")))?;
                    offset += want as u64;
                }
                let _ = tx.send(UploadFrame::Finished).await;
                Ok::<(), ClientError>(())
            };
            let (received, sent) =
                futures::join!(self.uploads.send_bytes(plan.upload_id, rx), sending);
            sent?;
            let received = received?;
            if !received.needed.is_empty() {
                return Err(ClientError::Stream(format!(
                    "{path}: {} range(s) did not arrive",
                    received.needed.len()
                )));
            }
        }
        Ok(self
            .uploads
            .complete(plan.upload_id, save.on_conflict)
            .await?)
    }

    /// Send the named ranges of `bytes`; the server's account of what is
    /// still missing afterwards.
    async fn send_ranges(
        &self,
        upload: UploadId,
        bytes: &[u8],
        ranges: &[ChunkRange],
    ) -> Result<Vec<ChunkRange>> {
        let (tx, rx) = vox::channel::<UploadFrame>();
        let frames: Vec<(u64, &[u8])> = ranges
            .iter()
            .flat_map(|r| {
                let (start, end) = (r.start as usize, (r.end as usize).min(bytes.len()));
                bytes[start..end]
                    .chunks(FRAME)
                    .enumerate()
                    .map(move |(i, piece)| ((start + i * FRAME) as u64, piece))
            })
            .collect();
        let sending = async move {
            for (offset, piece) in frames {
                if tx
                    .send(UploadFrame::Chunk {
                        offset,
                        bytes: piece.to_vec(),
                    })
                    .await
                    .is_err()
                {
                    // The call side reports why; stopping is all this
                    // half can do.
                    return;
                }
            }
            let _ = tx.send(UploadFrame::Finished).await;
        };
        let (received, ()) = futures::join!(self.uploads.send_bytes(upload, rx), sending);
        Ok(received?.needed)
    }

    // ── Reading ────────────────────────────────────────────────────

    /// The whole file. For anything large, prefer [`Self::read_to`].
    pub async fn get(&self, root: RootId, path: &str) -> Result<Vec<u8>> {
        let ticket = self.media.read(root, parse(path)?).await?;
        let mut out = Vec::with_capacity(ticket.length.unwrap_or(0) as usize);
        self.redeem(&ticket, None, |chunk| out.extend_from_slice(chunk))
            .await?;
        Ok(out)
    }

    /// Bytes `first..=last` of the file — a seek, not a download.
    pub async fn read_range(
        &self,
        root: RootId,
        path: &str,
        first: u64,
        last: u64,
    ) -> Result<Vec<u8>> {
        let ticket = self.media.read(root, parse(path)?).await?;
        let mut out = Vec::new();
        self.redeem(&ticket, Some(ByteRange { first, last }), |chunk| {
            out.extend_from_slice(chunk);
        })
        .await?;
        Ok(out)
    }

    /// Stream the file into `sink`, one frame at a time, holding none of
    /// it. Returns the byte count.
    pub async fn read_to(&self, root: RootId, path: &str, sink: impl FnMut(&[u8])) -> Result<u64> {
        let ticket = self.media.read(root, parse(path)?).await?;
        self.redeem(&ticket, None, sink).await
    }

    /// Redeem a ticket on the byte lane. The stream says how long it will
    /// be, and whether it finished — a short read is an error here, never
    /// a quietly truncated file.
    pub async fn redeem(
        &self,
        ticket: &ByteTicket,
        range: Option<ByteRange>,
        mut sink: impl FnMut(&[u8]),
    ) -> Result<u64> {
        let (tx, mut rx) = vox::channel::<ByteFrame>();
        let request = ByteRequest {
            token: ticket.token.clone(),
            range,
        };
        let reading = async move {
            let mut expected = None;
            let mut got = 0u64;
            loop {
                let frame = match rx.recv().await {
                    Ok(Some(frame)) => frame,
                    Ok(None) => {
                        return Err(ClientError::Stream(
                            "the byte stream closed before saying it was done".into(),
                        ));
                    }
                    Err(e) => return Err(ClientError::Transport(format!("{e:?}"))),
                };
                let mut owned = None;
                let _ = frame.map(|f| owned = Some(f));
                match owned {
                    Some(ByteFrame::Opened { length, .. }) => expected = Some(length),
                    Some(ByteFrame::Chunk { bytes, .. }) => {
                        got += bytes.len() as u64;
                        sink(&bytes);
                    }
                    Some(ByteFrame::Done) => {
                        return match expected {
                            Some(len) if len != got => Err(ClientError::Stream(format!(
                                "the stream promised {len} bytes and sent {got}"
                            ))),
                            _ => Ok(got),
                        };
                    }
                    Some(ByteFrame::Failed(fault)) => return Err(ClientError::Fault(fault)),
                    None => {}
                }
            }
        };
        drive(self.bytes.bytes(request, tx), reading).await
    }

    // ── References ─────────────────────────────────────────────────

    /// What a manifest's content reference — `(root, path)` and an
    /// optional pinned etag — points at now.
    pub async fn resolve(
        &self,
        root: RootId,
        path: &str,
        pinned: Option<ContentId>,
    ) -> Result<Resolved> {
        match self.tree.entry(root, parse(path)?).await {
            Ok(now) => Ok(match pinned {
                Some(pin) if now.content.as_ref() != Some(&pin) => {
                    Resolved::Moved { now, pinned: pin }
                }
                _ => Resolved::Current(now),
            }),
            Err(e) => match ClientError::from(e) {
                ClientError::Fault(FilesFault::PathNotFound(_)) => Ok(Resolved::Missing { pinned }),
                other => Err(other),
            },
        }
    }

    /// The bytes a reference means: the pinned content when it pinned
    /// some — by address, so a path overwritten since still yields the
    /// recording the manifest named — and otherwise whatever is at the
    /// path.
    pub async fn get_ref(
        &self,
        root: RootId,
        path: &str,
        pinned: Option<ContentId>,
    ) -> Result<Vec<u8>> {
        match self.resolve(root, path, pinned).await? {
            Resolved::Current(_) => self.get(root, path).await,
            Resolved::Moved { pinned, .. }
            | Resolved::Missing {
                pinned: Some(pinned),
            } => {
                let ticket = self.media.read_content(pinned).await?;
                let mut out = Vec::new();
                self.redeem(&ticket, None, |c| out.extend_from_slice(c))
                    .await?;
                Ok(out)
            }
            Resolved::Missing { pinned: None } => {
                Err(ClientError::Fault(FilesFault::PathNotFound(parse(path)?)))
            }
        }
    }

    // ── Live ───────────────────────────────────────────────────────

    /// Follow changes: `on_event` is called for each one until it returns
    /// `false` or the stream ends. `root` narrows to one root.
    ///
    /// Subscribe *before* reading current state, then fold events in — so
    /// nothing is missed between the read and the subscription.
    pub async fn events(
        stream: &TreeServiceStreamClient,
        root: Option<RootId>,
        mut on_event: impl FnMut(FilesEvent) -> bool,
    ) -> Result<()> {
        let (tx, mut rx) = vox::channel::<FilesEvent>();
        let listening = async move {
            loop {
                match rx.recv().await {
                    Ok(Some(frame)) => {
                        let mut owned = None;
                        let _ = frame.map(|e| owned = Some(e));
                        if let Some(event) = owned
                            && !on_event(event)
                        {
                            return Ok(());
                        }
                    }
                    Ok(None) => return Ok(()),
                    Err(e) => return Err(ClientError::Transport(format!("{e:?}"))),
                }
            }
        };
        drive(stream.events(root, tx), listening).await
    }
}

/// Run a subscription call and the loop reading its channel together,
/// finishing when the reader does.
///
/// Not `join!`: a subscription's call future may not resolve until the
/// server notices the channel is gone, which for a reader that stopped
/// early is the next event — possibly never. The reader decides when we
/// are done; the call is dropped then.
async fn drive<C, L>(call: C, listen: L) -> L::Output
where
    C: std::future::Future,
    L: std::future::Future,
{
    let call = std::pin::pin!(call);
    let listen = std::pin::pin!(listen);
    match futures::future::select(call, listen).await {
        futures::future::Either::Left((_, listen)) => listen.await,
        futures::future::Either::Right((out, _)) => out,
    }
}

/// A root's id as the lanes take it.
#[must_use]
pub fn root_id(root: &FileRootInfo) -> RootId {
    RootId::new(root.id)
}

fn parse(path: &str) -> Result<RootPath> {
    RootPath::parse(path).map_err(|e| ClientError::Fault(FilesFault::BadPath(e)))
}
