//! [`ChunkStore`]: the on-disk pairing of an iroh-blobs `FsStore` (chunk
//! bytes, content-addressed by blake3) with a manifests directory (Files'
//! own `FileId -> chunk list` records, kept outside iroh-blobs per
//! ADR 0001).

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use fastcdc::v2020::AsyncStreamCDC;
use futures::StreamExt;
use iroh_blobs::store::fs::options::Options;
use iroh_blobs::store::{GcConfig as IrohGcConfig, ProtectCb, ProtectOutcome};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::RwLock;

use crate::chunk::chunker::ChunkerConfig;
use crate::chunk::error::{Error, Result};
use crate::chunk::gc::{GcConfig, GcStats};
use crate::chunk::manifest::{ChunkRef, FileId, Manifest};

/// The chunk-store substrate: streaming write/read of files as
/// content-defined, deduplicated, blake3-addressed chunks.
///
/// A store lives at a directory with two subdirectories: `blobs/` (the
/// iroh-blobs `FsStore`) and `manifests/` (Files-owned, plain files named
/// `<file id hex>.manifest`). Both are required to resolve a `FileId`;
/// deleting `blobs/` and reconstructing it from a copy plus the manifests
/// (or vice versa, once GC is in place) is the "rebuildable" property ADR
/// 0001 asks for.
///
/// **GC pinning model (deliberate choice, not the iroh-blobs default):**
/// chunks are stored via [`iroh_blobs::api::blobs::Batch::temp_tag`] rather
/// than the default `.await` (which mints a *permanent* named `Tag` per
/// call — with ~1 chunk per MiB, a multi-GB file would leave thousands of
/// permanent rows in iroh-blobs' own tags table that nothing ever cleans
/// up). Per ADR 0001, liveness here is Files' own manifests, not iroh-blobs
/// tags/temp-tags — manifests are the roots (see the `gc` module doc for
/// how a store opened with GC enabled derives that liveness). Every
/// `TempTag` [`ChunkStore::write_stream`] mints for one call — new adds
/// *and* already-present chunks alike — is held until that call's manifest
/// is durable, then dropped together: each chunk needs protection for the
/// whole call, not just up to its own `add`, since it has no manifest
/// referencing it — and therefore nothing else keeping it alive — until
/// the call finishes.
pub struct ChunkStore {
    blobs: iroh_blobs::store::fs::FsStore,
    manifests_dir: PathBuf,
    /// The **whole-file tier**: content stored as one file per blob,
    /// named by its blake3 hash, placed by *linking* rather than
    /// copying — see [`ChunkStore::write_path`]. Sits beside `blobs/`
    /// rather than inside it because iroh-blobs owns that directory's
    /// layout and offers no link-based import.
    whole_dir: PathBuf,
    chunker_config: ChunkerConfig,
    /// Set only for a store opened via [`ChunkStore::open_with_gc`] — the
    /// gate [`ChunkStore::gc`] checks before doing anything, since a store
    /// opened without a GC interval has nothing that will ever reclaim the
    /// chunks a manifest removal would orphan. See the `gc` module doc:
    /// iroh-blobs' background task (wired up at open time when this is
    /// `true`) derives its own liveness live from the manifests directory
    /// on every sweep, so there's no protect-set *state* to hold here.
    chunk_gc_enabled: bool,
    /// Quiesce lock between [`ChunkStore::write_stream`] and the GC protect
    /// callback (only actually contended when GC is enabled — held
    /// uncontended otherwise). iroh-blobs' own mark-then-sweep pass takes a
    /// *snapshot* of live tags/temp-tags once, before sweeping — a chunk
    /// added and tagged strictly after that snapshot but before the
    /// subsequent sweep's blob listing is invisible to that pass' `live`
    /// set even though its temp tag exists at sweep time (confirmed the
    /// hard way: holding `TempTag`s for a whole `write_stream` call alone
    /// was not sufficient under sustained concurrent write/GC pressure in
    /// testing — a handful of chunks vanished despite their manifests
    /// staying intact). Rather than chase that timing inside iroh-blobs'
    /// internals, `write_stream` holds a read guard on this lock for its
    /// whole call, and the protect callback holds a write guard for the
    /// duration of its scan — so no `write_stream` call can ever be
    /// partway through (some chunks added, blob visible; others not) while
    /// a scan is in flight. Every write the callback's scan can observe is
    /// either fully durable (its manifest is on disk, findable) or not yet
    /// started (blocked on the lock); there is no partial state to miss.
    write_lock: Arc<RwLock<()>>,
}

impl ChunkStore {
    /// Open (creating if absent) a chunk store rooted at `root`, with no
    /// chunk-level GC — [`ChunkStore::gc`] on a store opened this way
    /// removes swept manifests but returns [`Error::GcDisabled`] rather
    /// than reclaiming chunks, since nothing would ever sweep them.
    pub async fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_config(root, ChunkerConfig::default()).await
    }

    /// Open a chunk store with a non-default [`ChunkerConfig`] (e.g. a
    /// smaller average chunk size for a root of many small text files) and
    /// no chunk-level GC — see [`ChunkStore::open`].
    pub async fn open_with_config(
        root: impl AsRef<Path>,
        chunker_config: ChunkerConfig,
    ) -> Result<Self> {
        Self::open_inner(root, chunker_config, None).await
    }

    /// Open a chunk store with chunk-level GC enabled: [`ChunkStore::gc`]
    /// will actually reclaim unreferenced chunks (on iroh-blobs' own
    /// background schedule, per the `gc` module doc — not synchronously
    /// within the `gc` call itself).
    pub async fn open_with_gc(
        root: impl AsRef<Path>,
        chunker_config: ChunkerConfig,
        gc: GcConfig,
    ) -> Result<Self> {
        Self::open_inner(root, chunker_config, Some(gc)).await
    }

    async fn open_inner(
        root: impl AsRef<Path>,
        chunker_config: ChunkerConfig,
        gc: Option<GcConfig>,
    ) -> Result<Self> {
        chunker_config.validate()?;
        let root = root.as_ref();
        let blobs_dir = root.join("blobs");
        let manifests_dir = root.join("manifests");
        let whole_dir = root.join("whole");
        tokio::fs::create_dir_all(&blobs_dir).await?;
        tokio::fs::create_dir_all(&manifests_dir).await?;
        tokio::fs::create_dir_all(&whole_dir).await?;

        let chunk_gc_enabled = gc.is_some();
        let write_lock: Arc<RwLock<()>> = Arc::new(RwLock::new(()));
        let mut options = Options::new(&blobs_dir);
        if let Some(gc) = gc {
            // Derives liveness live, from what's actually on disk, on every
            // invocation — see the `gc` module doc for why this (not a
            // snapshot `ChunkStore::gc` publishes) is what closes the
            // stale-protect-set data-loss window.
            //
            // Any read failure here — listing the directory, or reading
            // one manifest — aborts the *whole pass* rather than skipping
            // just the one file that failed. This has to be conservative:
            // a permanently corrupt manifest and a manifest that's fine but
            // hit a transient I/O hiccup on this particular scan are
            // indistinguishable from here, and treating the latter as "no
            // chunks to protect for it" is exactly how a perfectly healthy
            // manifest's chunk gets swept out from under it — irreversibly,
            // since a chunk deletion can't be undone by the *next* pass
            // reading it correctly. `gc` itself never decodes a *kept*
            // manifest's contents (only enumerates the directory for
            // `FileId`/mtime), so a corrupt manifest still can't wedge
            // anything durable — it just means this pass reclaims nothing
            // and tries again next interval.
            let manifests_dir_for_gc = manifests_dir.clone();
            let write_lock_for_gc = write_lock.clone();
            let add_protected: ProtectCb = Arc::new(move |live: &mut HashSet<iroh_blobs::Hash>| {
                let manifests_dir = manifests_dir_for_gc.clone();
                let write_lock = write_lock_for_gc.clone();
                Box::pin(async move {
                    // Quiesce every `write_stream` call for the duration of
                    // this scan (see `write_lock`'s doc on `ChunkStore` for
                    // why this, not iroh-blobs' own temp-tag bookkeeping,
                    // is what actually closes the concurrent-write race).
                    let _quiesce = write_lock.write().await;
                    let Ok(manifests) = manifests_with_mtime_in(&manifests_dir).await else {
                        return ProtectOutcome::Abort;
                    };
                    for (file_id, _mtime) in &manifests {
                        let Ok(manifest) = read_manifest_in(&manifests_dir, *file_id).await else {
                            return ProtectOutcome::Abort;
                        };
                        live.extend(
                            manifest
                                .chunks
                                .iter()
                                .map(|chunk| iroh_blobs::Hash::from(chunk.hash)),
                        );
                    }
                    ProtectOutcome::Continue
                })
            });
            options.gc = Some(IrohGcConfig {
                interval: gc.interval,
                add_protected: Some(add_protected),
            });
        }

        let db_path = blobs_dir.join("blobs.db");
        let blobs = iroh_blobs::store::fs::FsStore::load_with_opts(db_path, options)
            .await
            .map_err(|e| {
                Error::Store(format!(
                    "opening blob store at {}: {e}",
                    blobs_dir.display()
                ))
            })?;
        Ok(Self {
            blobs,
            manifests_dir,
            whole_dir,
            chunker_config,
            chunk_gc_enabled,
            write_lock,
        })
    }

    /// Removes every manifest that isn't in `protected` (the caller's
    /// externally-referenced set — Vault-referenced versions, from the
    /// version-store layer above) and is older than `keep_newer` (guards a
    /// manifest written concurrently with this call, mirroring
    /// `Backend::gc`'s own `keep_newer` contract) — the *only* thing this
    /// method does. Requires a store opened with [`ChunkStore::open_with_gc`]
    /// (otherwise [`Error::GcDisabled`]): its removals are only meaningful
    /// because that store's background task is what actually reclaims the
    /// chunks a removed manifest orphans (see the `gc` module doc for why
    /// that reclamation is asynchronous and driven independently, not
    /// published by this call).
    pub async fn gc(
        &self,
        protected: &BTreeSet<FileId>,
        keep_newer: SystemTime,
    ) -> Result<GcStats> {
        if !self.chunk_gc_enabled {
            return Err(Error::GcDisabled);
        }

        let mut manifests_swept = 0usize;
        for (file_id, mtime) in self.manifests_with_mtime().await? {
            let keep = protected.contains(&file_id) || mtime >= keep_newer;
            if !keep {
                self.remove_manifest(file_id).await?;
                manifests_swept += 1;
            }
        }

        // The whole tier has no background collector of its own —
        // iroh-blobs sweeps only what it stores — so its blobs are
        // reclaimed here, synchronously, from the same authority:
        // whatever the surviving manifests still name is live.
        //
        // Only meaningful when something was actually swept, and
        // ordered after the removals so the scan cannot see a manifest
        // that is about to disappear.
        if manifests_swept > 0 {
            self.sweep_whole().await?;
        }

        Ok(GcStats { manifests_swept })
    }

    /// Delete whole-tier blobs no surviving manifest references.
    ///
    /// Reads the live set first and deletes second: a blob written
    /// between the two is missed by this pass and collected by the
    /// next, which is the safe direction to be wrong in.
    ///
    /// A manifest that cannot be read aborts the sweep — deleting
    /// nothing, returning `Ok(0)` — rather than failing. Both halves
    /// matter. Not deleting, because a transient read error and a
    /// genuinely unreferenced blob are indistinguishable from here and
    /// a deletion cannot be undone by the next pass reading correctly
    /// (the same conservatism the chunk protect callback applies). Not
    /// failing, because `gc()` deliberately does not decode the
    /// manifests it keeps: one corrupt file used to wedge every other
    /// manifest's removal, permanently, since its mtime never changes
    /// (`a_corrupt_kept_manifest_does_not_wedge_gc`).
    async fn sweep_whole(&self) -> Result<usize> {
        let mut live: HashSet<blake3::Hash> = HashSet::new();
        for (file_id, _) in self.manifests_with_mtime().await? {
            let Ok(manifest) = self.read_manifest(file_id).await else {
                return Ok(0);
            };
            live.extend(manifest.chunks.iter().map(|c| c.hash));
        }

        let mut removed = 0usize;
        let mut shards = tokio::fs::read_dir(&self.whole_dir).await?;
        while let Some(shard) = shards.next_entry().await? {
            if !shard.file_type().await?.is_dir() {
                continue;
            }
            let mut inner = tokio::fs::read_dir(shard.path()).await?;
            while let Some(sub) = inner.next_entry().await? {
                if !sub.file_type().await?.is_dir() {
                    continue;
                }
                let mut blobs = tokio::fs::read_dir(sub.path()).await?;
                while let Some(blob) = blobs.next_entry().await? {
                    let name = blob.file_name();
                    let Some(name) = name.to_str() else { continue };
                    // Skip in-flight temp names (`<hash>.tmp.<pid>.<n>`).
                    let Ok(hash) = blake3::Hash::from_hex(name) else {
                        continue;
                    };
                    if !live.contains(&hash) {
                        tokio::fs::remove_file(blob.path()).await?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    /// Every manifest currently on disk, with its last-modified time (the
    /// `keep_newer` protection signal, mirroring
    /// `ObjectStore::list_with_mtime` in the version-store crate).
    async fn manifests_with_mtime(&self) -> Result<Vec<(FileId, SystemTime)>> {
        manifests_with_mtime_in(&self.manifests_dir).await
    }

    async fn remove_manifest(&self, file_id: FileId) -> Result<()> {
        let path = self.manifest_path(file_id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// The number of distinct chunk blobs currently in the store — a cheap
    /// observability hook, and how `gc`'s test suite observes iroh-blobs'
    /// background sweep actually reclaiming a chunk (see the `gc` module
    /// doc: reclamation happens on iroh-blobs' own schedule, not
    /// synchronously within `ChunkStore::gc`).
    pub async fn chunk_count(&self) -> Result<usize> {
        let hashes = self
            .blobs
            .blobs()
            .list()
            .hashes()
            .await
            .map_err(|e| Error::Store(format!("listing chunks: {e}")))?;
        Ok(hashes.len())
    }

    /// Stream `source` into the store: chunk it with FastCDC, blake3-hash
    /// each chunk, and put chunks the blob store doesn't already have
    /// (existing chunks are skipped — this is where cross-save dedup
    /// happens). Returns the resulting file's content address.
    ///
    /// Bounded memory: `source` is never read into a single buffer. At
    /// most one chunk (at most `chunker_config.max_size` bytes) is held at
    /// a time, so this is safe to call on a multi-GB source.
    // t[impl files.scale.small-files] — "identical content is stored once,
    // and re-referencing it transfers nothing": skipping chunks the blob
    // store already holds is both halves at once, since a re-reference
    // finds every chunk present and writes none
    pub async fn write_stream<R>(&self, source: R) -> Result<FileId>
    where
        R: AsyncRead + Unpin + Send,
    {
        let mut chunker = AsyncStreamCDC::new(
            source,
            self.chunker_config.min_size,
            self.chunker_config.avg_size,
            self.chunker_config.max_size,
        );
        let mut stream = std::pin::pin!(chunker.as_stream());

        // The real protection against a concurrent GC pass (see
        // `write_lock`'s doc on `ChunkStore`): held for this whole call, so
        // no GC scan can observe this write partway through. Acquired
        // before any chunk touches the blob store.
        let _write_guard = self.write_lock.read().await;

        // Belt-and-suspenders on top of `write_lock`, and iroh-blobs' own
        // recommended pattern regardless: protects every chunk this call
        // touches — newly added *and* already-present — via its tagging
        // mechanism too. Each `TempTag` protects its blob only as long as
        // the `TempTag` value itself is held (a `Batch`'s own lifetime does
        // *not* keep them alive on its own — `#[must_use]`, confirmed the
        // hard way), so every one returned below is collected here and
        // dropped together only after `write_manifest` durably references
        // every chunk in it.
        let batch = self
            .blobs
            .batch()
            .await
            .map_err(|e| Error::Store(format!("opening a gc-protection batch: {e}")))?;
        let mut pending_tags = Vec::new();

        let mut chunks: Vec<ChunkRef> = Vec::new();
        while let Some(item) = stream.next().await {
            let data = item.map_err(|e| Error::Io(e.into()))?.data;
            let hash = blake3::hash(&data);
            let len = data.len() as u64;
            let already_present = self
                .blobs
                .has(*hash.as_bytes())
                .await
                .map_err(|e| Error::Store(format!("checking chunk {hash}: {e}")))?;
            let temp_tag = if already_present {
                batch
                    .temp_tag(iroh_blobs::Hash::from(hash))
                    .await
                    .map_err(|e| Error::Store(format!("protecting existing chunk {hash}: {e}")))?
            } else {
                batch
                    .add_bytes(data)
                    .await
                    .map_err(|e| Error::Store(format!("storing chunk {hash}: {e}")))?
            };
            pending_tags.push(temp_tag);
            chunks.push(ChunkRef { hash, len });
        }

        let manifest = Manifest::new(chunks);
        let file_id = manifest.file_id();
        self.write_manifest(file_id, &manifest).await?;
        drop(pending_tags);
        drop(batch);
        Ok(file_id)
    }

    /// Store the file at `path`, returning its content address — the
    /// entry point for content that is already a file on disk, as
    /// opposed to [`ChunkStore::write_stream`]'s arbitrary reader.
    ///
    /// **The bytes are linked, not copied.** When the file and this
    /// store are on the same filesystem, the content enters the store
    /// as a second directory entry for the *same data*: a reflink
    /// (`FICLONE`) where the filesystem can clone extents, otherwise a
    /// plain hardlink. Either way the store gains a full, independent
    /// reference to the content for **zero additional space**.
    ///
    /// This is what makes importing an existing archive possible at
    /// all. Copying is bounded by free space — a 5 TB tree needs 5 TB
    /// to version — while linking is bounded by nothing: the bytes are
    /// already on the disk.
    ///
    /// It also delivers the property that matters most here: **the
    /// content cannot be lost by deleting the original.** A hardlink
    /// keeps the inode alive, so removing the live file (by hand, by
    /// accident, or by an app) frees nothing and the stored version
    /// still reads back byte-for-byte. Measured on the production
    /// array: 300 MB linked costs 0 MB, and deleting the source frees
    /// 0 MB while the store returns the identical hash.
    ///
    /// What a hardlink does NOT protect against is a program rewriting
    /// the file **in place** — one inode, so both change together.
    /// A reflink does protect against that (separate inodes sharing
    /// extents, copy-on-write), which is why it is tried first, but it
    /// is an optimization: the deployment is not required to provide a
    /// cloning filesystem.
    ///
    /// Chunking still exists for the case linking cannot serve — a
    /// source on a *different* filesystem, where the content must be
    /// copied and content-defined chunks at least earn cross-version
    /// dedup for the copy. That choice is made by comparing device
    /// ids, which is deterministic and cheap, so
    /// [`ChunkStore::probe_path`] can predict it exactly (see its doc
    /// for why that matters).
    pub async fn write_path(&self, path: impl AsRef<Path>) -> Result<FileId> {
        let path = path.as_ref();
        let meta = tokio::fs::metadata(path).await?;
        let len = meta.len();
        if !self.wants_whole(path, len).await {
            let file = tokio::fs::File::open(path).await?;
            return self.write_stream(file).await;
        }

        // Hash first: the destination is named by content, so it has to
        // be known before the link is made. One read pass, bounded
        // memory — the same pass `probe_path` performs.
        let hash = Self::hash_file(path).await?;
        let dest = self.whole_path(&hash);

        // No GC interaction to guard: the whole tier is swept from the
        // same manifest-derived liveness as chunks (see `gc`), and the
        // manifest below is written before this call returns, exactly
        // like `write_stream`'s.
        // Already stored (and the right length — a short file at a
        // content address is damage from a crashed write, repaired by
        // placing again rather than trusted).
        let stored = tokio::fs::metadata(&dest)
            .await
            .map(|m| m.len() == len)
            .unwrap_or(false);
        if !stored {
            let src = path.to_path_buf();
            let dst = dest.clone();
            tokio::task::spawn_blocking(move || place_whole(&src, &dst))
                .await
                .map_err(|e| Error::Io(std::io::Error::other(e)))??;
        }

        // Register the placed file with iroh-blobs **by reference**, so
        // the blob has an outboard — the BLAKE3 tree over its content.
        //
        // That is what makes a transfer of it verifiable in flight: with
        // an outboard, any range can be sent with the proof that it
        // belongs to this hash, and a receiver rejects a bad window
        // rather than discovering it after the last byte. Without one,
        // the only check available on an 800 GB take is at the end.
        //
        // By reference, so nothing is copied: iroh-blobs opens the file
        // where it lies and stores the outboard, which is a few hundred
        // KB for a file of any size. The link this just made is what
        // makes that safe to reference — it is the store's own path,
        // unaffected by anything the user does to their tree.
        self.reference_whole(&dest).await?;

        let manifest = Manifest::new(vec![ChunkRef { hash, len }]);
        let file_id = manifest.file_id();
        self.write_manifest(file_id, &manifest).await?;
        Ok(file_id)
    }

    /// Register an already-placed whole-tier file with iroh-blobs by
    /// reference, computing its outboard.
    ///
    /// Idempotent and best-effort in one specific way: a store that
    /// cannot compute an outboard still holds the content and can still
    /// serve it unverified, so this logs rather than failing a write that
    /// otherwise succeeded. What it costs is in-flight verification for
    /// that one blob, which is worth strictly less than the bytes.
    async fn reference_whole(&self, placed: &Path) -> Result<()> {
        use iroh_blobs::BlobFormat;
        use iroh_blobs::api::blobs::AddPathOptions;
        use iroh_blobs::api::proto::ImportMode;

        let outcome = self
            .blobs
            .add_path_with_opts(AddPathOptions {
                path: placed.to_path_buf(),
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            })
            .temp_tag()
            .await;
        match outcome {
            Ok(tag) => {
                // The tag drops here, exactly as `import_chunk`'s does:
                // liveness is the manifest, never a tags row.
                drop(tag);
                Ok(())
            }
            Err(err) => {
                tracing::warn!(
                    path = %placed.display(),
                    %err,
                    "files-store: no outboard for this blob — transfers of it \
                     cannot be verified per range"
                );
                Ok(())
            }
        }
    }

    /// Which byte ranges of `hash` this store already holds.
    ///
    /// The resume cursor, and iroh-blobs' own: a partially-received blob
    /// records which ranges arrived, verified, durably. That is a better
    /// answer than a length — an interrupted transfer can have gaps if
    /// windows ever land out of order — and it is state this store does
    /// not have to keep itself.
    ///
    /// An empty set means nothing of it is here, which is also what an
    /// unknown hash returns: "no ranges" and "no such blob" are the same
    /// instruction to a caller, namely fetch all of it.
    pub async fn have_ranges(&self, hash: blake3::Hash) -> Result<bao_tree::ChunkRanges> {
        // A whole-tier file that predates outboards, or one whose
        // outboard could not be computed, is present in full and unknown
        // to iroh-blobs. Reporting "nothing" would re-fetch a file that
        // is already here.
        if let Ok(meta) = tokio::fs::metadata(self.whole_path(&hash)).await {
            if self.blobs.has(*hash.as_bytes()).await.unwrap_or(false) {
                // Known to blobs: its own bitfield is authoritative.
            } else {
                return Ok(bao_tree::ChunkRanges::from(
                    ..bao_tree::ChunkNum::chunks(meta.len()),
                ));
            }
        }
        match self.blobs.observe(*hash.as_bytes()).await {
            Ok(bitfield) => Ok(bitfield.ranges),
            Err(_) => Ok(bao_tree::ChunkRanges::empty()),
        }
    }

    /// Bao-encode `ranges` of `hash`: the bytes, plus the proof they
    /// belong to that hash.
    ///
    /// The serving half of a verified transfer. BLAKE3 is a Merkle tree,
    /// so a range can carry the hashes on its path to the root — which is
    /// what lets a receiver reject a corrupt window immediately instead
    /// of after the file.
    pub async fn export_ranges(
        &self,
        hash: blake3::Hash,
        ranges: bao_tree::ChunkRanges,
    ) -> Result<Vec<u8>> {
        self.blobs
            .export_bao(*hash.as_bytes(), ranges)
            .bao_to_vec()
            .await
            .map_err(|e| Error::Store(format!("bao export of {hash}: {e}")))
    }

    /// Verify received bao-encoded ranges and write them.
    ///
    /// Verification is not a separate step a caller could skip: the
    /// encoding carries the proof, so a window that does not hash into
    /// `hash` is refused here and nothing is written. A transfer that
    /// stops part-way leaves exactly the ranges that verified, which is
    /// what [`ChunkStore::have_ranges`] then reports.
    pub async fn import_ranges(
        &self,
        hash: blake3::Hash,
        ranges: bao_tree::ChunkRanges,
        bao: Vec<u8>,
    ) -> Result<()> {
        let _write_guard = self.write_lock.read().await;
        self.blobs
            .import_bao_bytes(iroh_blobs::Hash::from(*hash.as_bytes()), ranges, bao)
            .await
            .map_err(|e| Error::Store(format!("bao import of {hash}: {e}")))
    }

    /// Derive the [`FileId`] the file at `path` *would* have in this
    /// store, writing nothing — [`ChunkStore::write_path`]'s pure twin,
    /// exactly as [`ChunkStore::probe_stream`] is `write_stream`'s.
    ///
    /// It must make the **same whole-vs-chunked decision** `write_path`
    /// would: the two produce different ids for identical bytes, so a
    /// probe that chunked what the write would link (or the reverse)
    /// would report "changed" on every capture of an untouched file —
    /// turning the skip-what-is-unchanged fast path into a guaranteed
    /// re-import of the whole tree, forever. That is why the decision
    /// is a pure function of (device id, size) and never of whether a
    /// link call happens to succeed.
    pub async fn probe_path(&self, path: impl AsRef<Path>) -> Result<FileId> {
        let path = path.as_ref();
        let len = tokio::fs::metadata(path).await?.len();
        if !self.wants_whole(path, len).await {
            let file = tokio::fs::File::open(path).await?;
            return self.probe_stream(file).await;
        }
        let hash = Self::hash_file(path).await?;
        Ok(Manifest::new(vec![ChunkRef { hash, len }]).file_id())
    }

    /// Whole-file placement applies when the source can actually be
    /// linked into the store — same filesystem — and the file is at
    /// least `whole_file_threshold` bytes (0 by default: everything,
    /// since a link costs nothing regardless of size).
    ///
    /// Deliberately decided from metadata alone. See `probe_path`.
    async fn wants_whole(&self, path: &Path, len: u64) -> bool {
        if len < self.chunker_config.whole_file_threshold {
            return false;
        }
        same_filesystem(path, &self.whole_dir).await
    }

    /// blake3 of a file's contents, read in bounded memory.
    async fn hash_file(path: &Path) -> Result<blake3::Hash> {
        use tokio::io::AsyncReadExt as _;
        let file = tokio::fs::File::open(path).await?;
        let mut reader = tokio::io::BufReader::new(file);
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize())
    }

    /// Where a whole-stored blob lives. Sharded two levels: one flat
    /// directory would hold an entry per file in every root, and a
    /// media archive has hundreds of thousands.
    fn whole_path(&self, hash: &blake3::Hash) -> PathBuf {
        let hex = hash.to_hex();
        self.whole_dir
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(hex.as_str())
    }

    /// Read a window of one stored chunk.
    ///
    /// The serving half of a resumable transfer. Both tiers seek, so a
    /// window of a whole-tier blob costs the window rather than the
    /// blob — which is the difference between serving a 244 GB file and
    /// taking the server down with it.
    pub async fn read_chunk_range<W>(
        &self,
        hash: blake3::Hash,
        offset: u64,
        len: u64,
        dest: &mut W,
    ) -> Result<u64>
    where
        W: AsyncWrite + Unpin,
    {
        use tokio::io::AsyncReadExt as _;

        if let Some(mut file) = self.open_whole(&hash).await? {
            tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(offset))
                .await
                .map_err(Error::Io)?;
            return tokio::io::copy(&mut file.take(len), dest)
                .await
                .map_err(Error::Io);
        }
        let mut reader = self.blobs.reader(*hash.as_bytes());
        tokio::io::AsyncSeekExt::seek(&mut reader, std::io::SeekFrom::Start(offset))
            .await
            .map_err(Error::Io)?;
        tokio::io::copy(&mut reader.take(len), dest)
            .await
            .map_err(Error::Io)
    }

    /// Derive the [`FileId`] `source` *would* have in this store,
    /// writing nothing — no chunks, no manifest, no locks. The pure
    /// half of [`ChunkStore::write_stream`]: same chunker config, same
    /// per-chunk blake3, same manifest encoding, so the answer is
    /// exactly the id a real write of the same bytes returns. This is
    /// what lets an is-this-content-already-versioned comparison run
    /// without persisting never-versioned bytes as orphaned store data
    /// (PR #289 review).
    pub async fn probe_stream<R>(&self, source: R) -> Result<FileId>
    where
        R: AsyncRead + Unpin + Send,
    {
        let mut chunker = AsyncStreamCDC::new(
            source,
            self.chunker_config.min_size,
            self.chunker_config.avg_size,
            self.chunker_config.max_size,
        );
        let mut stream = std::pin::pin!(chunker.as_stream());
        let mut chunks: Vec<ChunkRef> = Vec::new();
        while let Some(item) = stream.next().await {
            let data = item.map_err(|e| Error::Io(e.into()))?.data;
            chunks.push(ChunkRef {
                hash: blake3::hash(&data),
                len: data.len() as u64,
            });
        }
        Ok(Manifest::new(chunks).file_id())
    }

    /// Stream the file named by `file_id` to `dest`, one chunk at a time.
    /// Bounded memory: chunks are copied to `dest` and dropped as they are
    /// read, never assembled into a whole-file buffer.
    pub async fn read_to<W>(&self, file_id: FileId, dest: &mut W) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let manifest = self.read_manifest(file_id).await?;
        for chunk in &manifest.chunks {
            // The whole tier first: a linked blob lives as a plain file
            // under `whole/`, not in iroh-blobs (see `write_path`).
            if let Some(mut file) = self.open_whole(&chunk.hash).await? {
                let copied = tokio::io::copy(&mut file, dest).await.map_err(Error::Io)?;
                if copied != chunk.len {
                    return Err(Error::MissingChunk(chunk.hash.to_hex().to_string()));
                }
                continue;
            }
            let hash_bytes = *chunk.hash.as_bytes();
            let mut reader = self.blobs.reader(hash_bytes);
            let copied = match tokio::io::copy(&mut reader, dest).await {
                Ok(copied) => copied,
                Err(io_err) => {
                    // Distinguish "chunk genuinely absent from the blob
                    // store" (repairable — re-fetch/re-derive it) from a
                    // real I/O fault, so #257's version-store layer can
                    // tell the two apart instead of treating everything
                    // as fatal.
                    let present = self.blobs.has(hash_bytes).await.unwrap_or(true);
                    if present {
                        return Err(Error::Io(io_err));
                    }
                    return Err(Error::MissingChunk(chunk.hash.to_hex().to_string()));
                }
            };
            if copied != chunk.len {
                return Err(Error::MissingChunk(chunk.hash.to_hex().to_string()));
            }
        }
        Ok(())
    }

    /// Convenience wrapper over [`ChunkStore::read_to`] that collects the
    /// whole file into memory. For tests and small files — large files
    /// should use `read_to` against a sink (a file, a network stream)
    /// directly.
    pub async fn read_to_vec(&self, file_id: FileId) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.read_to(file_id, &mut buf).await?;
        Ok(buf)
    }

    /// Total byte length of a file's content — the sum of its chunk
    /// lengths, read from the manifest (no chunk bytes touched). Serving
    /// an HTTP `Content-Range` needs the total up front.
    pub async fn content_len(&self, file_id: FileId) -> Result<u64> {
        Ok(self
            .read_manifest(file_id)
            .await?
            .chunks
            .iter()
            .map(|c| c.len)
            .sum())
    }

    /// Write the half-open byte range `[start, start + len)` of a file's
    /// content to `dest`, reading only the chunks that overlap the
    /// window — so an HTTP Range request (a `<video>` seek) doesn't read
    /// the whole file. A window past the end is clamped. Chunk reads are
    /// bounded memory (one chunk at a time, like `read_to`).
    pub async fn read_range<W>(
        &self,
        file_id: FileId,
        start: u64,
        len: u64,
        dest: &mut W,
    ) -> Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        use tokio::io::AsyncReadExt as _;
        let manifest = self.read_manifest(file_id).await?;
        let end = start.saturating_add(len); // exclusive
        let mut offset = 0u64; // start of the current chunk in the file
        for chunk in &manifest.chunks {
            let chunk_start = offset;
            let chunk_end = offset + chunk.len;
            offset = chunk_end;
            // Skip chunks entirely before or after the window.
            if chunk_end <= start || chunk_start >= end {
                continue;
            }
            // Overlap of [start,end) with this chunk, relative to it.
            let from = start.saturating_sub(chunk_start);
            let to = end.min(chunk_end) - chunk_start;

            // Seek within the blob rather than reading it whole: with a
            // whole-file blob (see `write_path`) one "chunk" IS the
            // entire multi-hundred-GB file, and buffering it to serve a
            // 1 MB video seek would take the server down. Both tiers
            // are seekable, so only the window is ever read.
            let want = to - from;
            if let Some(mut file) = self.open_whole(&chunk.hash).await? {
                tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(from))
                    .await
                    .map_err(Error::Io)?;
                let copied = tokio::io::copy(&mut file.take(want), dest)
                    .await
                    .map_err(Error::Io)?;
                if copied != want {
                    return Err(Error::MissingChunk(chunk.hash.to_hex().to_string()));
                }
                continue;
            }
            let mut reader = self.blobs.reader(*chunk.hash.as_bytes());
            tokio::io::AsyncSeekExt::seek(&mut reader, std::io::SeekFrom::Start(from))
                .await
                .map_err(Error::Io)?;
            let copied = tokio::io::copy(&mut reader.take(want), dest)
                .await
                .map_err(Error::Io)?;
            // Short read where the manifest promised bytes: the same
            // "absent chunk, not an I/O fault" distinction `read_to`
            // draws, so a repairable store doesn't look like a broken one.
            if copied != want {
                return Err(Error::MissingChunk(chunk.hash.to_hex().to_string()));
            }
        }
        Ok(())
    }

    /// Fetch the manifest for `file_id`, if this store has it.
    pub async fn manifest(&self, file_id: FileId) -> Result<Manifest> {
        self.read_manifest(file_id).await
    }

    /// Is this one chunk in the blob store? The chunk-level presence
    /// probe replica reconcile plans transfers with (issue #264):
    /// "resumable at chunk level" means asking this per chunk and
    /// fetching only the misses.
    pub async fn has_chunk(&self, hash: blake3::Hash) -> Result<bool> {
        if tokio::fs::metadata(self.whole_path(&hash)).await.is_ok() {
            return Ok(true);
        }
        self.blobs
            .has(*hash.as_bytes())
            .await
            .map_err(|e| Error::Store(format!("checking chunk {hash}: {e}")))
    }

    /// Open a whole-tier blob, or `None` when this hash is not stored
    /// that way (the ordinary chunked case).
    async fn open_whole(&self, hash: &blake3::Hash) -> Result<Option<tokio::fs::File>> {
        match tokio::fs::File::open(self.whole_path(hash)).await {
            Ok(file) => Ok(Some(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Read one chunk's bytes. Chunks are bounded by the chunker's max
    /// size, so a whole-chunk `Vec` is bounded memory by construction.
    pub async fn read_chunk(&self, hash: blake3::Hash) -> Result<Vec<u8>> {
        if let Some(mut file) = self.open_whole(&hash).await? {
            let mut buf = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut file, &mut buf)
                .await
                .map_err(Error::Io)?;
            return Ok(buf);
        }
        let hash_bytes = *hash.as_bytes();
        let mut reader = self.blobs.reader(hash_bytes);
        let mut buf = Vec::new();
        match tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await {
            Ok(_) => Ok(buf),
            Err(io_err) => {
                let present = self.blobs.has(hash_bytes).await.unwrap_or(true);
                if present {
                    Err(Error::Io(io_err))
                } else {
                    Err(Error::MissingChunk(hash.to_hex().to_string()))
                }
            }
        }
    }

    /// A held guard that quiesces the GC protect scan for its lifetime —
    /// the seam replica sync (issue #264) holds across a file's whole
    /// chunk+manifest import so a chunk that has arrived but whose
    /// manifest has not yet landed cannot be swept out from under the
    /// import (PR #291 review). It is the SAME `write_lock.read()`
    /// [`ChunkStore::write_stream`] holds for exactly this reason —
    /// the protect callback takes `write_lock.write()`, so any read
    /// guard blocks a sweep — just held across the import instead of a
    /// single call. Owned so the caller can hold it across `.await`s.
    pub async fn gc_quiesce_guard(&self) -> tokio::sync::OwnedRwLockReadGuard<()> {
        self.write_lock.clone().read_owned().await
    }

    /// Store one chunk received from a peer, **verified**: the bytes are
    /// hashed here and a payload that doesn't hash to `expected` is
    /// refused — a sync peer is never trusted about content addresses
    /// (issue #264's "iroh verified streaming" property, applied at this
    /// store's boundary).
    ///
    /// The chunk has NO manifest referencing it yet, so it has no GC
    /// protection of its own — the caller must hold a
    /// [`ChunkStore::gc_quiesce_guard`] across the whole file import
    /// (chunks + manifest) so nothing sweeps it before its manifest
    /// lands (PR #291 review).
    pub async fn import_chunk(&self, expected: blake3::Hash, bytes: Vec<u8>) -> Result<()> {
        let actual = blake3::hash(&bytes);
        if actual != expected {
            return Err(Error::Store(format!(
                "chunk payload hashes to {actual}, peer claimed {expected}"
            )));
        }
        let _write_guard = self.write_lock.read().await;
        let batch = self
            .blobs
            .batch()
            .await
            .map_err(|e| Error::Store(format!("opening a gc-protection batch: {e}")))?;
        let _tag = batch
            .add_bytes(bytes)
            .await
            .map_err(|e| Error::Store(format!("storing chunk {expected}: {e}")))?;
        // The tag drops with the batch: liveness comes from the manifest
        // the caller imports once every chunk is present (manifests are
        // the GC roots).
        Ok(())
    }

    /// Store a manifest received from a peer, returning its `FileId` —
    /// **refused unless every chunk it references is already present**,
    /// so an imported manifest never names content this store cannot
    /// serve (a partial replica stays honest: absent files are stubs,
    /// never half-manifests).
    pub async fn import_manifest(&self, manifest: &Manifest) -> Result<FileId> {
        for chunk in &manifest.chunks {
            if !self.has_chunk(chunk.hash).await? {
                return Err(Error::MissingChunk(chunk.hash.to_hex().to_string()));
            }
        }
        self.write_manifest_unchecked(manifest).await
    }

    /// Write a manifest without requiring its chunks to be here.
    ///
    /// [`Self::import_manifest`]'s presence check is the right rule for
    /// a store that will serve the file: a manifest promising bytes the
    /// store cannot produce is a store that lies when read.
    ///
    /// A host holding an org's *structure* is the case that rule does
    /// not fit (`files.peering.replication`). Its manifests are true —
    /// they carry the file's real size and the real hashes of its
    /// chunks — and the bytes are simply somewhere else. That is a stub
    /// at the store layer, and refusing to record it would mean such a
    /// host could not say how big a project is.
    ///
    /// What keeps it honest is that nothing on such a host serves those
    /// bytes: a root with no working tree refuses content reads before
    /// reaching the store, and its listings mark every entry as not
    /// resident. Use this only where that is true.
    pub async fn write_manifest_unbacked(&self, manifest: &Manifest) -> Result<FileId> {
        self.write_manifest_unchecked(manifest).await
    }

    async fn write_manifest_unchecked(&self, manifest: &Manifest) -> Result<FileId> {
        let file_id = manifest.file_id();
        self.write_manifest(file_id, manifest).await?;
        Ok(file_id)
    }

    /// Whether a manifest for `file_id` is on disk. Does not verify that
    /// every chunk it references is still present in the blob store.
    pub async fn has(&self, file_id: FileId) -> bool {
        self.read_manifest(file_id).await.is_ok()
    }

    /// Flush the blob store to disk. iroh-blobs' `FsStore` may not
    /// durably persist the last few seconds of writes without this
    /// (see the crate's own `fs` module docs) — call it before a process
    /// exit or before relying on the store surviving a crash.
    pub async fn shutdown(&self) -> Result<()> {
        self.blobs
            .shutdown()
            .await
            .map_err(|e| Error::Store(format!("shutdown: {e}")))
    }

    fn manifest_path(&self, file_id: FileId) -> PathBuf {
        manifest_path_in(&self.manifests_dir, file_id)
    }

    /// Durably write `manifest` at `file_id`'s path. If a file already
    /// exists there *and decodes*, it is necessarily byte-identical (the
    /// path is derived from the content hash of the manifest bytes), so
    /// there is nothing to do beyond refreshing its mtime (see below). If
    /// it exists but fails to decode — e.g. a prior write crashed between
    /// `rename` and this process' next start, on a filesystem where rename
    /// can be observed before the data it pointed at is durable — that is
    /// treated as damage to repair, not a reason to skip the write:
    /// without this, `read_to` for that `FileId` would fail forever.
    async fn write_manifest(&self, file_id: FileId, manifest: &Manifest) -> Result<()> {
        let path = self.manifest_path(file_id);
        if let Ok(existing) = tokio::fs::read(&path).await {
            if Manifest::decode(&existing).is_ok() {
                // `ChunkStore::gc`'s `keep_newer` protection relies on
                // mtime reflecting the most recent write, not just the
                // first one — a caller re-`write_stream`ing already-stored
                // content is exactly the "written concurrently with a gc
                // pass" case that contract exists to protect (mirrors
                // `ObjectStore::write`'s identical fix in the version-store
                // crate).
                match self.touch_manifest(&path).await {
                    Ok(()) => return Ok(()),
                    Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                        // Raced a concurrent `gc()` sweep of this exact
                        // manifest between the read above and this touch —
                        // the file we just confirmed exists (and decodes)
                        // is gone. The bytes are still known-good (we
                        // decoded them a moment ago), so this is a
                        // legitimate rewrite, not corruption: fall through
                        // to the normal write-then-rename path below rather
                        // than surfacing an io::Error for what the caller
                        // sees as a successful write.
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // Write-then-fsync-then-rename, plus an fsync of the containing
        // directory: on ext4/btrfs a rename can be observed durable before
        // the data blocks it points at are, so skipping the file fsync
        // (or the directory fsync after the rename) can leave a durable
        // but corrupt manifest behind a power loss. The temp name must be
        // unique per *call*, not just per file id: two concurrent
        // write_stream calls for identical content otherwise share one
        // tmp path, and the loser's rename fails with ENOENT because the
        // winner's rename already consumed it out from under it.
        static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = self.manifests_dir.join(format!(
            "{}.manifest.tmp.{}.{unique}",
            file_id.to_hex(),
            std::process::id()
        ));
        {
            let mut file = tokio::fs::File::create(&tmp_path).await?;
            file.write_all(&manifest.encode()).await?;
            file.sync_all().await?;
        }
        tokio::fs::rename(&tmp_path, &path).await?;
        Self::fsync_dir(&self.manifests_dir).await?;
        Ok(())
    }

    async fn fsync_dir(dir: &Path) -> Result<()> {
        let dir = tokio::fs::File::open(dir).await?;
        dir.sync_all().await?;
        Ok(())
    }

    /// Set `path`'s mtime to now (see `ObjectStore::touch` for the same
    /// pattern: `std::fs::File::set_modified` has no tokio-native
    /// equivalent, so this hands the already-open, already I/O-completed
    /// file handle to a blocking thread).
    async fn touch_manifest(&self, path: &Path) -> Result<()> {
        let file = tokio::fs::OpenOptions::new().write(true).open(path).await?;
        let std_file = file.into_std().await;
        tokio::task::spawn_blocking(move || std_file.set_modified(SystemTime::now()))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))??;
        Ok(())
    }

    async fn read_manifest(&self, file_id: FileId) -> Result<Manifest> {
        read_manifest_in(&self.manifests_dir, file_id).await
    }
}

fn manifest_path_in(dir: &Path, file_id: FileId) -> PathBuf {
    dir.join(format!("{}.manifest", file_id.to_hex()))
}

/// Are `a` and `b` on the same filesystem? Decides whether the store
/// can link a source in rather than copy it. `b`'s directory is used
/// when `b` itself does not exist yet.
///
/// A device-id comparison rather than a trial link, because the answer
/// has to be identical for `probe_path` and `write_path` — see
/// `probe_path`'s doc. On anything that cannot report a device, the
/// answer is "no": copying is always correct, just slower.
async fn same_filesystem(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let Ok(am) = tokio::fs::metadata(a).await else {
            return false;
        };
        let Ok(bm) = tokio::fs::metadata(b).await else {
            return false;
        };
        am.dev() == bm.dev()
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

/// How a whole-file blob's bytes got into the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Extents shared, separate inodes — free, and an in-place rewrite
    /// of the source copies-on-write rather than touching the stored
    /// version.
    Reflinked,
    /// Same inode, second directory entry — free, and the content
    /// survives deletion of the original. An in-place rewrite of the
    /// source is visible through both.
    Hardlinked,
    /// A real copy. Correct everywhere, and the only option when the
    /// filesystem refuses both links despite reporting the same device
    /// (a bind mount of a different subvolume, a full disk, an
    /// exhausted link count).
    Copied,
}

/// Place `src`'s content at `dst` as cheaply as the filesystem allows:
/// reflink, else hardlink, else copy. Blocking; call from
/// `spawn_blocking`.
///
/// Writes through a unique temp name and renames, so a crash midway
/// cannot leave a short file sitting at a content address — the same
/// discipline `write_manifest` uses. (A reflink or hardlink is atomic
/// in itself, but the rename costs nothing and keeps one path for all
/// three cases.)
fn place_whole(src: &Path, dst: &Path) -> Result<Placement> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = dst
        .parent()
        .ok_or_else(|| Error::Store(format!("whole-file path {} has no parent", dst.display())))?;
    std::fs::create_dir_all(parent)?;

    let unique = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(
        "{}.tmp.{}.{unique}",
        dst.file_name().and_then(|n| n.to_str()).unwrap_or("blob"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);

    // Reflink first, and a hard link only where there is no reflink.
    //
    // The order matters for correctness, not speed. A reflink is
    // copy-on-write: the store's blob is unaffected by anything done to
    // the user's file afterwards. A hard link is the same inode, so a
    // rewrite **in place** — which `std::fs::write` performs, and which
    // any number of programs do — changes the bytes under a blob that
    // is named by their hash. The old version is then gone, and a
    // version store that loses old versions has lost the argument.
    //
    // This was inverted once, to buy back space on a NAS: over NFS 4.2
    // `reflink` *succeeds* and duplicates — the client turns it into a
    // server-side COPY, so a 2.5 GB project became 5.0 GB with nothing
    // reporting a copy. Hard-linking fixed the space and broke the
    // promise; `read_at_serves_the_version_asked_for` catches exactly
    // that, by rewriting a file in place and asking for the first
    // version back.
    //
    // So the space problem on a filesystem with no real reflink is not
    // solved here, and is not solved by linking. It is a genuine cost of
    // keeping history where the filesystem cannot share blocks.
    let placement = if reflink_copy::reflink(src, &tmp).is_ok() {
        Placement::Reflinked
    } else if std::fs::hard_link(src, &tmp).is_ok() {
        Placement::Hardlinked
    } else {
        std::fs::copy(src, &tmp)?;
        Placement::Copied
    };

    match std::fs::rename(&tmp, dst) {
        Ok(()) => Ok(placement),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(Error::Io(e))
        }
    }
}

/// Free-function twin of `ChunkStore::manifests_with_mtime`, taking the
/// manifests directory directly rather than `&self` — the GC protect
/// callback built in `open_inner` needs this (and [`read_manifest_in`])
/// without holding a `ChunkStore` reference, since the callback is
/// constructed *during* `open_inner`, before `Self` exists.
async fn manifests_with_mtime_in(dir: &Path) -> Result<Vec<(FileId, SystemTime)>> {
    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(hex) = name.strip_suffix(".manifest") else {
            continue; // skip temp files (`.manifest.tmp.<pid>.<n>`)
        };
        let Ok(file_id) = FileId::from_hex(hex) else {
            continue;
        };
        let metadata = entry.metadata().await?;
        out.push((file_id, metadata.modified()?));
    }
    Ok(out)
}

/// Free-function twin of `ChunkStore::read_manifest` — see
/// [`manifests_with_mtime_in`]'s doc for why the GC protect callback needs
/// this shape.
async fn read_manifest_in(dir: &Path, file_id: FileId) -> Result<Manifest> {
    let path = manifest_path_in(dir, file_id);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::UnknownFileId(file_id.to_hex()));
        }
        Err(e) => return Err(Error::Io(e)),
    };
    Manifest::decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test (data-loss finding): `write_manifest`'s dedup fast
    /// path reads + decodes an existing manifest, then calls
    /// `touch_manifest` to refresh its mtime — but nothing prevented a
    /// concurrent `remove_manifest` (what `ChunkStore::gc` calls on a swept
    /// manifest) from deleting that exact file in between, which used to
    /// surface as `Error::Io(NotFound)` from a logically-successful,
    /// content-already-durable write. `write_manifest` now falls through to
    /// a full rewrite on that specific NotFound instead of propagating it.
    /// This hammers the race probabilistically (no single interleaving is
    /// forceable through the public API) rather than asserting one exact
    /// ordering.
    #[tokio::test]
    async fn write_manifest_survives_concurrent_manifest_removal_pressure() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ChunkStore::open(dir.path()).await.unwrap());
        let content = b"racing a concurrent manifest removal".to_vec();
        let file_id = store.write_stream(&content[..]).await.unwrap();

        let remover = {
            let store = store.clone();
            tokio::spawn(async move {
                for _ in 0..200 {
                    store.remove_manifest(file_id).await.unwrap();
                    tokio::task::yield_now().await;
                }
            })
        };
        let writer = {
            let store = store.clone();
            let content = content.clone();
            tokio::spawn(async move {
                for _ in 0..200 {
                    store.write_stream(&content[..]).await.unwrap();
                    tokio::task::yield_now().await;
                }
            })
        };
        let (remover, writer) = tokio::join!(remover, writer);
        remover.unwrap();
        writer.unwrap();

        // One final write must leave the store in a consistent, readable
        // state regardless of how the race above landed.
        let final_id = store.write_stream(&content[..]).await.unwrap();
        assert_eq!(final_id, file_id);
        assert!(
            store.has(file_id).await,
            "the manifest must survive a final write after the race"
        );
        assert_eq!(store.read_to_vec(file_id).await.unwrap(), content);
    }

    #[tokio::test]
    async fn read_to_reports_missing_chunk_not_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChunkStore::open(dir.path()).await.unwrap();

        // A manifest naming a chunk that was never written to the blob
        // store — simulates a chunk that's absent (evicted, not yet
        // hydrated, or a corrupt write) without needing a delete API
        // iroh-blobs doesn't expose to us.
        let fake_chunk = ChunkRef {
            hash: blake3::hash(b"this chunk was never stored"),
            len: 4,
        };
        let manifest = Manifest::new(vec![fake_chunk]);
        let file_id = manifest.file_id();
        store.write_manifest(file_id, &manifest).await.unwrap();

        let mut sink = tokio::io::sink();
        let err = store.read_to(file_id, &mut sink).await.unwrap_err();
        assert!(
            matches!(err, Error::MissingChunk(_)),
            "expected Error::MissingChunk for an absent chunk, got {err:?}"
        );
    }

    #[tokio::test]
    async fn write_stream_does_not_mint_persistent_tags() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChunkStore::open(dir.path()).await.unwrap();
        // Several MiB so this writes multiple chunks, not just one.
        let content: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        store.write_stream(&content[..]).await.unwrap();

        let tag_count = store.blobs.tags().list().await.unwrap().count().await;
        assert_eq!(
            tag_count, 0,
            "write_stream must not leave persistent iroh-blobs tags behind — \
             Files' manifests are the liveness authority, not the tags table"
        );
    }

    #[tokio::test]
    async fn write_manifest_repairs_a_corrupt_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChunkStore::open(dir.path()).await.unwrap();
        let content = b"repair me if I show up corrupt on disk";

        let file_id = store.write_stream(&content[..]).await.unwrap();
        // Simulate the crash scenario: a manifest file exists at the
        // right path but its bytes are garbage (e.g. rename observed
        // before the data was durable).
        let path = store.manifest_path(file_id);
        tokio::fs::write(&path, b"not a valid manifest")
            .await
            .unwrap();
        assert!(Manifest::decode(&tokio::fs::read(&path).await.unwrap()).is_err());

        // Re-writing the same content must repair it rather than
        // early-returning past the corruption.
        let repaired_id = store.write_stream(&content[..]).await.unwrap();
        assert_eq!(repaired_id, file_id);
        assert_eq!(store.read_to_vec(file_id).await.unwrap(), content);
    }

    /// `content_len` + `read_range` (the `<video>` seek path): the total
    /// matches, a full range reads the whole file, and windows in the
    /// middle / straddling the end return exactly the right bytes across
    /// chunk boundaries.
    #[tokio::test]
    async fn read_range_returns_the_right_window() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChunkStore::open(dir.path()).await.unwrap();
        // Big + varied so it splits into many chunks and a window crosses
        // chunk boundaries (the case the seek path cares about).
        let content: Vec<u8> = (0..1_000_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 16) as u8)
            .collect();
        let file_id = store.write_stream(&content[..]).await.unwrap();

        assert_eq!(
            store.content_len(file_id).await.unwrap(),
            content.len() as u64
        );

        let mut full = Vec::new();
        store
            .read_range(file_id, 0, content.len() as u64, &mut full)
            .await
            .unwrap();
        assert_eq!(full, content, "full range == whole file");

        for (start, len) in [(0u64, 10u64), (12_345, 40_000), (999_990, 50)] {
            let mut got = Vec::new();
            store
                .read_range(file_id, start, len, &mut got)
                .await
                .unwrap();
            let end = (start + len).min(content.len() as u64) as usize;
            assert_eq!(got, content[start as usize..end], "window {start}+{len}");
        }
    }
}
