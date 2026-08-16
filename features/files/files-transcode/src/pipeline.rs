//! [`TranscodePipeline`] — the lazy generate-and-cache engine (issue
//! #269). It ties a [`crate::store::RenditionStore`] (where renditions
//! live) to a [`crate::transcoder::Transcoder`] (what makes them) and a
//! source reader (the chunk store the source content lives in).
//!
//! Lazy by default: [`TranscodePipeline::rendition`] returns a cached
//! rendition if present, else generates it once and caches it — so a
//! request that misses generates exactly once (AC 2). A checkpoint
//! trigger calls [`TranscodePipeline::warm_up`] to build a media file's
//! whole ladder ahead of demand (AC 1). Generation stages the source to
//! a temp file first, because ffmpeg cannot read the CAS directly.

use std::sync::Arc;

use files_store::chunk::{ChunkStore, FileId};

use crate::error::{Error, Result};
use crate::recipe::{MediaClass, RECIPE_VERSION, RenditionKind};
use crate::store::{RenditionKey, RenditionStore};
use crate::transcoder::Transcoder;

/// A generated (or cached) rendition: its CAS id, length, and MIME.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendition {
    pub file_id: FileId,
    pub len: u64,
    pub kind: RenditionKind,
}

pub struct TranscodePipeline {
    /// Where SOURCE content lives — renditions are generated from it.
    source_chunks: Arc<ChunkStore>,
    /// The rendition store — `Arc` so it is opened ONCE per root and
    /// shared across requests. Opening a second one on the same dir
    /// (its private iroh-blobs `FsStore`) while the first is alive
    /// hangs, so the backend caches this and builds pipelines around it.
    store: Arc<RenditionStore>,
    transcoder: Arc<dyn Transcoder>,
}

impl TranscodePipeline {
    #[must_use]
    pub fn new(
        source_chunks: Arc<ChunkStore>,
        store: Arc<RenditionStore>,
        transcoder: Arc<dyn Transcoder>,
    ) -> Self {
        Self {
            source_chunks,
            store,
            transcoder,
        }
    }

    /// The rendition store, for GC and direct reads.
    #[must_use]
    pub fn store(&self) -> &RenditionStore {
        &self.store
    }

    /// One rendition for `source_file_id`, generated on demand and
    /// cached (AC 2). `None` return is impossible — a miss generates —
    /// but a `kind` that doesn't apply to the source's media class (a
    /// filmstrip of an audio file) is a [`Error::NotMedia`].
    pub async fn rendition(
        &self,
        source_file_id: &FileId,
        kind: RenditionKind,
    ) -> Result<Rendition> {
        let key = RenditionKey {
            source_file_id: &source_file_id.to_hex(),
            recipe_version: RECIPE_VERSION,
            kind,
        };
        if let Some((file_id, len)) = self.store.get(&key).await? {
            return Ok(Rendition { file_id, len, kind });
        }
        // Miss: stage the source, classify, refuse an inapplicable kind,
        // then generate + cache exactly once.
        let staged = self.stage_source(source_file_id).await?;
        let class = self.transcoder.probe(staged.path()).await?;
        if !RenditionKind::ladder_for(class).contains(&kind) {
            return Err(Error::NotMedia(format!(
                "{kind:?} does not apply to a {class:?} source"
            )));
        }
        let bytes = self.transcoder.generate(kind, staged.path()).await?;
        let file_id = self.store.put(&key, &bytes).await?;
        Ok(Rendition {
            file_id,
            len: bytes.len() as u64,
            kind,
        })
    }

    /// Generate a source's whole rendition ladder ahead of demand — the
    /// checkpoint-trigger warm-up (AC 1). Returns the kinds generated
    /// (empty for a non-media source). Idempotent: an already-cached
    /// rendition is left alone.
    pub async fn warm_up(&self, source_file_id: &FileId) -> Result<Vec<RenditionKind>> {
        let staged = self.stage_source(source_file_id).await?;
        let class = self.transcoder.probe(staged.path()).await?;
        let mut done = Vec::new();
        for &kind in RenditionKind::ladder_for(class) {
            let key = RenditionKey {
                source_file_id: &source_file_id.to_hex(),
                recipe_version: RECIPE_VERSION,
                kind,
            };
            if self.store.get(&key).await?.is_some() {
                done.push(kind);
                continue;
            }
            let bytes = self.transcoder.generate(kind, staged.path()).await?;
            self.store.put(&key, &bytes).await?;
            done.push(kind);
        }
        Ok(done)
    }

    /// The media class of a source (probed) — so a caller can decide
    /// whether to warm it up at all without generating anything.
    pub async fn classify(&self, source_file_id: &FileId) -> Result<MediaClass> {
        let staged = self.stage_source(source_file_id).await?;
        self.transcoder.probe(staged.path()).await
    }

    /// Stage a source's bytes from the CAS to a temp file — ffmpeg reads
    /// paths, not the chunk store. The bytes are STREAMED chunk-by-chunk
    /// into the file (never buffered whole in RAM — a source is a
    /// multi-GB video). The temp file lives as long as the returned
    /// guard; a `tokio` file handle written to its path streams into it.
    async fn stage_source(&self, source_file_id: &FileId) -> Result<tempfile::NamedTempFile> {
        let tmp = tempfile::NamedTempFile::new()?;
        let mut file = tokio::fs::File::create(tmp.path()).await?;
        self.source_chunks
            .read_to(*source_file_id, &mut file)
            .await?;
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        Ok(tmp)
    }
}
