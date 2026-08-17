//! [`RenditionStore`] — where derived renditions live (issue #269).
//!
//! Rendition content lives in its **own** CAS chunk store, separate
//! from the source content's — because a rendition is in no commit
//! tree, so the version store's own GC (which sweeps manifests not
//! reachable from a protected commit) would otherwise treat every
//! rendition as garbage. Keeping renditions in a private store makes
//! this store the sole authority on their lifetime.
//!
//! The mapping from a rendition **key** `(source FileId, recipe
//! version, kind)` to the rendition's `FileId` is a small on-disk index
//! — one file per rendition, named `<source hex>.<recipe>.<kind>.json`.
//! That naming is what makes the two GC rules cheap directory scans:
//!
//! - **GC-tied to the source** (AC 3): a rendition whose source content
//!   is no longer in the source store is swept — derived media
//!   outliving its source is pure waste. A protected source keeps its
//!   renditions.
//! - **Recipe bump doesn't orphan** (AC 4): a rendition whose recipe
//!   version isn't the current one is swept too, so a bump regenerates
//!   under the new key and the old entries don't leak.
//!
//! Sweeping an index entry also reclaims its content: [`RenditionStore::gc`]
//! runs the rendition chunk store's own `gc` with the surviving
//! renditions as the protect set, so a removed rendition's manifest and
//! chunks are collected.

use std::path::PathBuf;
use std::sync::Arc;

use files_store::chunk::{ChunkStore, ChunkerConfig, FileId, GcConfig};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::recipe::{RECIPE_VERSION, RenditionKind};

/// One rendition's index record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RenditionRecord {
    source_file_id: String,
    recipe_version: u32,
    kind: String,
    /// CAS `FileId` (hex) of the rendition's bytes.
    rendition_file_id: String,
    /// Rendition byte length, for a served `Content-Length`.
    len: u64,
}

/// A rendition key: what content, which recipe, which kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenditionKey<'a> {
    pub source_file_id: &'a str,
    pub recipe_version: u32,
    pub kind: RenditionKind,
}

impl RenditionKey<'_> {
    fn filename(&self) -> String {
        format!(
            "{}.{}.{}.json",
            self.source_file_id,
            self.recipe_version,
            self.kind.tag()
        )
    }
}

/// The rendition store: a private GC-enabled chunk store for rendition
/// content plus the key→FileId index.
pub struct RenditionStore {
    chunks: Arc<ChunkStore>,
    index_dir: PathBuf,
}

impl RenditionStore {
    /// Open (creating if absent) a rendition store rooted at `dir`: a
    /// private `content/` chunk store (GC-enabled — renditions are a
    /// cache, reclaimed by [`RenditionStore::gc`]) beside an `index/`
    /// dir. `dir` is typically `<root store>/renditions`.
    pub async fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        let chunks = ChunkStore::open_with_gc(
            dir.join("content"),
            ChunkerConfig::default(),
            GcConfig::default(),
        )
        .await?;
        let index_dir = dir.join("index");
        tokio::fs::create_dir_all(&index_dir).await?;
        Ok(Self {
            chunks: Arc::new(chunks),
            index_dir,
        })
    }

    fn record_path(&self, key: &RenditionKey<'_>) -> PathBuf {
        self.index_dir.join(key.filename())
    }

    /// The CAS `FileId` + byte length of a cached rendition, or `None`
    /// when it hasn't been generated. Verifies the content is actually
    /// present (an index entry whose CAS content was swept reads as
    /// absent, so it regenerates rather than 404ing forever).
    pub async fn get(&self, key: &RenditionKey<'_>) -> Result<Option<(FileId, u64)>> {
        let path = self.record_path(key);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = tokio::fs::read(&path).await?;
        let record: RenditionRecord =
            serde_json::from_slice(&bytes).map_err(|e| Error::Index(e.to_string()))?;
        let file_id = FileId::from_hex(&record.rendition_file_id)
            .map_err(|e| Error::Index(format!("{}: {e}", record.rendition_file_id)))?;
        if !self.chunks.has(file_id).await {
            return Ok(None);
        }
        Ok(Some((file_id, record.len)))
    }

    /// Store a freshly generated rendition's bytes in the CAS and record
    /// the index entry, returning the rendition's CAS `FileId`.
    pub async fn put(&self, key: &RenditionKey<'_>, bytes: &[u8]) -> Result<FileId> {
        let file_id = self.chunks.write_stream(bytes).await?;
        let record = RenditionRecord {
            source_file_id: key.source_file_id.to_string(),
            recipe_version: key.recipe_version,
            kind: key.kind.tag().to_string(),
            rendition_file_id: file_id.to_hex(),
            len: bytes.len() as u64,
        };
        let path = self.record_path(key);
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&record).map_err(|e| Error::Index(e.to_string()))?,
        )
        .await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(file_id)
    }

    /// Stream a cached rendition's bytes to `dest`.
    pub async fn read_to<W>(&self, file_id: FileId, dest: &mut W) -> Result<()>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        self.chunks.read_to(file_id, dest).await?;
        Ok(())
    }

    /// Whether the rendition content is present — GC observability.
    pub async fn has_content(&self, file_id: FileId) -> bool {
        self.chunks.has(file_id).await
    }

    /// A rendition's total byte length — for a served `Content-Range`.
    pub async fn content_len(&self, file_id: FileId) -> Result<u64> {
        Ok(self.chunks.content_len(file_id).await?)
    }

    /// Stream a byte range `[start, start + len)` of a rendition to
    /// `dest` — the `<video>`-seek path (issue #270), reading only the
    /// overlapping chunks.
    pub async fn read_range<W>(
        &self,
        file_id: FileId,
        start: u64,
        len: u64,
        dest: &mut W,
    ) -> Result<()>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        self.chunks.read_range(file_id, start, len, dest).await?;
        Ok(())
    }

    /// Every distinct source `FileId` hex the index references — so a
    /// caller can resolve liveness for all of them ASYNC up front and
    /// hand [`RenditionStore::gc`] a plain synchronous predicate (no
    /// nested `block_on`).
    pub async fn source_ids(&self) -> Result<Vec<String>> {
        use std::collections::BTreeSet;
        let mut out: BTreeSet<String> = BTreeSet::new();
        let mut dir = tokio::fs::read_dir(&self.index_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.ends_with(".json") || name.ends_with(".json.tmp") {
                continue;
            }
            if let Ok(bytes) = tokio::fs::read(entry.path()).await
                && let Ok(record) = serde_json::from_slice::<RenditionRecord>(&bytes)
            {
                out.insert(record.source_file_id);
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Prune renditions no longer worth keeping (issue #269's two GC
    /// rules), returning how many index entries were removed. `is_live`
    /// answers whether a source `FileId` hex is still protected; a
    /// rendition is kept only when its source is live AND its recipe is
    /// the current [`RECIPE_VERSION`]. The surviving renditions'
    /// content is protected and everything else's manifest + chunks are
    /// reclaimed by the rendition store's own `gc`.
    pub async fn gc(&self, is_live: impl Fn(&str) -> bool) -> Result<u64> {
        use std::collections::BTreeSet;
        let mut removed = 0u64;
        let mut protected: BTreeSet<FileId> = BTreeSet::new();
        let mut dir = tokio::fs::read_dir(&self.index_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.ends_with(".json") || name.ends_with(".json.tmp") {
                continue;
            }
            let bytes = tokio::fs::read(entry.path()).await?;
            let record: RenditionRecord = match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                // A malformed index entry is junk — remove it.
                Err(_) => {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                    removed += 1;
                    continue;
                }
            };
            let keep = record.recipe_version == RECIPE_VERSION && is_live(&record.source_file_id);
            if keep {
                if let Ok(fid) = FileId::from_hex(&record.rendition_file_id) {
                    protected.insert(fid);
                }
            } else {
                tokio::fs::remove_file(entry.path()).await?;
                removed += 1;
            }
        }
        // Reclaim the removed renditions' content: keep_newer=now means
        // "protect nothing on recency" so a just-orphaned manifest is
        // eligible — the `protected` set is the sole keep signal here.
        self.chunks
            .gc(&protected, std::time::SystemTime::now())
            .await?;
        Ok(removed)
    }
}
