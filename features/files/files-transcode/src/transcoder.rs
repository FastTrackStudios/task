//! The [`Transcoder`] — the ffmpeg driver, behind a trait so the
//! caching / GC / recipe logic is testable without ffmpeg or real
//! media (issue #269). The real [`ffmpeg::FfmpegTranscoder`] shells out
//! to ffmpeg/ffprobe (behind the `ffmpeg` feature); tests inject a
//! deterministic fake.

use std::path::Path;

use crate::error::Result;
use crate::recipe::{MediaClass, RenditionKind};

/// Produces derived media from a source file on disk. Implementations
/// are the swappable half of the pipeline: the real one runs ffmpeg,
/// the test one fabricates deterministic bytes.
#[async_trait::async_trait]
pub trait Transcoder: Send + Sync {
    /// Classify the source so the pipeline knows which ladder to build.
    async fn probe(&self, source: &Path) -> Result<MediaClass>;

    /// Generate one rendition's bytes from `source`. The pipeline
    /// stages the source to a temp file first, so this always gets a
    /// real path (ffmpeg cannot read the CAS directly).
    async fn generate(&self, kind: RenditionKind, source: &Path) -> Result<Vec<u8>>;
}

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;

/// A deterministic in-process [`Transcoder`] for tests: it never runs
/// ffmpeg. `probe` classifies by a tiny magic prefix the test writes
/// (`VIDEO`/`AUDIO`), and `generate` returns bytes derived from the
/// source content + kind, so a rendition is reproducible and a recipe
/// change (simulated by prefixing) yields different bytes.
#[cfg(any(test, feature = "test-transcoder"))]
pub struct FakeTranscoder;

#[cfg(any(test, feature = "test-transcoder"))]
#[async_trait::async_trait]
impl Transcoder for FakeTranscoder {
    async fn probe(&self, source: &Path) -> Result<MediaClass> {
        let bytes = tokio::fs::read(source).await?;
        Ok(if bytes.starts_with(b"VIDEO") {
            MediaClass::Video
        } else if bytes.starts_with(b"AUDIO") {
            MediaClass::Audio
        } else {
            MediaClass::Other
        })
    }

    async fn generate(&self, kind: RenditionKind, source: &Path) -> Result<Vec<u8>> {
        let bytes = tokio::fs::read(source).await?;
        // Deterministic "rendition": the kind tag + a hash of the source
        // — same source + kind ⇒ same bytes (so caching is observable),
        // different source or kind ⇒ different bytes.
        let mut out = format!("{}:", kind.tag()).into_bytes();
        out.extend_from_slice(blake3::hash(&bytes).to_hex().as_bytes());
        Ok(out)
    }
}
