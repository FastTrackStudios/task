//! Streaming FastCDC (v2020) chunking over an [`AsyncRead`] source.
//!
//! There is no generic "pump a chunker and call back into async code"
//! combinator here on purpose: an `impl AsyncFnMut` parameter capturing
//! borrowed state produces a future whose `Send`-ness the compiler cannot
//! prove for every lifetime, which makes any caller built on it fail to
//! compile under `tokio::spawn`. [`ChunkStore::write_stream`] and
//! [`chunk_to_vec`] each drive their own `AsyncStreamCDC` loop directly
//! instead.

use fastcdc::v2020::AsyncStreamCDC;
use futures::StreamExt;
use tokio::io::AsyncRead;

use crate::chunk::error::{Error, Result};

/// FastCDC v2020 chunk-size policy. `min`/`max` follow the crate's own
/// convention of `avg / 4` and `avg * 4`; construct with
/// [`ChunkerConfig::with_avg_size`] rather than setting all three by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkerConfig {
    pub min_size: usize,
    pub avg_size: usize,
    pub max_size: usize,
    /// Size at or above which a file imported *from a path*
    /// ([`ChunkStore::write_path`]) is stored **whole** — one blob,
    /// *linked* into the store rather than copied — instead of being
    /// split into content-defined chunks.
    ///
    /// **Zero by default: every file, whatever its size.** A link costs
    /// nothing regardless of length, so there is no size at which
    /// copying-and-chunking becomes the better trade. The knob remains
    /// so a caller can force chunking (`u64::MAX`) — which is how the
    /// tests measure what linking actually saves.
    ///
    /// Only consulted by the path-taking entry points, and only when the
    /// source is on the same filesystem as the store; `write_stream` has
    /// no file to link and always chunks.
    pub whole_file_threshold: u64,
}

impl ChunkerConfig {
    /// 1 MiB average chunk size — a reasonable default for multi-GB DAW
    /// sessions and video media; callers versioning many small text files
    /// may want a smaller average.
    pub const DEFAULT_AVG_SIZE: usize = 1024 * 1024;

    /// Zero — link everything that can be linked.
    ///
    /// This was 64 MiB when whole-file placement meant a reflink, on the
    /// theory that small files should keep chunk-level dedup. Measuring
    /// a real import killed that theory: one 6.36 GiB song held only
    /// 1.35 GiB in files above the threshold, so 72% of it was still
    /// copied, and a 5 TB archive would still have needed terabytes it
    /// did not have. Since a link costs nothing at any size, the
    /// threshold had no work left to do.
    pub const DEFAULT_WHOLE_FILE_THRESHOLD: u64 = 0;

    pub fn with_avg_size(avg_size: usize) -> Self {
        Self {
            min_size: avg_size / 4,
            avg_size,
            max_size: avg_size * 4,
            whole_file_threshold: Self::DEFAULT_WHOLE_FILE_THRESHOLD,
        }
    }

    /// Store files of `bytes` and up whole (see
    /// [`ChunkerConfig::whole_file_threshold`]). `u64::MAX` disables the
    /// whole-file path entirely — everything chunks, as before it existed.
    pub fn with_whole_file_threshold(mut self, bytes: u64) -> Self {
        self.whole_file_threshold = bytes;
        self
    }

    /// Check `min_size`/`avg_size`/`max_size` against the bounds
    /// `fastcdc::v2020::AsyncStreamCDC` requires. `AsyncStreamCDC::new`
    /// only `debug_assert!`s these — in a release build an out-of-range
    /// config doesn't fail loudly, it corrupts chunking — so anything
    /// that didn't come from [`ChunkerConfig::with_avg_size`] (a caller
    /// building the struct literal directly, e.g. to try a very small
    /// average for many small files) must be validated before it reaches
    /// the chunker.
    pub fn validate(&self) -> Result<()> {
        use fastcdc::v2020::{
            AVERAGE_MAX, AVERAGE_MIN, MAXIMUM_MAX, MAXIMUM_MIN, MINIMUM_MAX, MINIMUM_MIN,
        };
        if !(MINIMUM_MIN..=MINIMUM_MAX).contains(&self.min_size) {
            return Err(Error::InvalidConfig(format!(
                "chunker min_size {} out of range [{MINIMUM_MIN}, {MINIMUM_MAX}]",
                self.min_size
            )));
        }
        if !(AVERAGE_MIN..=AVERAGE_MAX).contains(&self.avg_size) {
            return Err(Error::InvalidConfig(format!(
                "chunker avg_size {} out of range [{AVERAGE_MIN}, {AVERAGE_MAX}]",
                self.avg_size
            )));
        }
        if !(MAXIMUM_MIN..=MAXIMUM_MAX).contains(&self.max_size) {
            return Err(Error::InvalidConfig(format!(
                "chunker max_size {} out of range [{MAXIMUM_MIN}, {MAXIMUM_MAX}]",
                self.max_size
            )));
        }
        if !(self.min_size < self.avg_size && self.avg_size < self.max_size) {
            return Err(Error::InvalidConfig(format!(
                "chunker sizes must satisfy min < avg < max, got {} < {} < {}",
                self.min_size, self.avg_size, self.max_size
            )));
        }
        Ok(())
    }
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self::with_avg_size(Self::DEFAULT_AVG_SIZE)
    }
}

/// Collect `source`'s content-defined chunks into memory, per `config`.
/// For tests and small inputs only: this holds every chunk at once, which
/// is fine for a proptest-sized input but would defeat the bounded-memory
/// guarantee [`ChunkStore::write_stream`] relies on for large files, so it
/// drives its own loop rather than sharing one with this function.
pub async fn chunk_to_vec<R>(source: R, config: ChunkerConfig) -> Result<Vec<Vec<u8>>>
where
    R: AsyncRead + Unpin + Send,
{
    // fastcdc only `debug_assert!`s these bounds — in a release build an
    // out-of-range config wouldn't fail loudly, it would silently corrupt
    // chunking, so this public entry point must validate before handing
    // the config to AsyncStreamCDC::new.
    config.validate()?;
    let mut chunker =
        AsyncStreamCDC::new(source, config.min_size, config.avg_size, config.max_size);
    let mut stream = std::pin::pin!(chunker.as_stream());
    let mut chunks = Vec::new();
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(|e| Error::Io(e.into()))?;
        chunks.push(chunk.data);
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chunks_reassemble_to_original_bytes() {
        // Enough bytes, with enough variation, to cross several chunk
        // boundaries at the default average size.
        let mut data = Vec::new();
        for i in 0..200_000u32 {
            data.extend_from_slice(&i.to_le_bytes());
        }
        let config = ChunkerConfig::with_avg_size(16 * 1024);
        let chunks = chunk_to_vec(&data[..], config).await.unwrap();
        assert!(
            chunks.len() > 1,
            "expected multiple chunks for 800KB of varied input"
        );
        let reassembled: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(reassembled, data);
    }

    #[tokio::test]
    async fn empty_source_yields_no_chunks() {
        let chunks = chunk_to_vec(&b""[..], ChunkerConfig::default())
            .await
            .unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn default_config_validates() {
        ChunkerConfig::default().validate().unwrap();
    }

    #[test]
    fn tiny_avg_size_fails_validation() {
        assert!(ChunkerConfig::with_avg_size(10).validate().is_err());
    }

    #[test]
    fn zeroed_config_fails_validation() {
        let config = ChunkerConfig {
            min_size: 0,
            avg_size: 0,
            max_size: 0,
            whole_file_threshold: ChunkerConfig::DEFAULT_WHOLE_FILE_THRESHOLD,
        };
        assert!(config.validate().is_err());
    }

    /// `chunk_to_vec` is the entry point a caller can hand a hand-built
    /// `ChunkerConfig` to — it must reject an invalid one itself rather
    /// than handing it to `AsyncStreamCDC::new`, which only
    /// `debug_assert!`s the bounds and would silently corrupt chunking in
    /// a release build.
    #[tokio::test]
    async fn rejects_invalid_config_instead_of_reaching_the_chunker() {
        let config = ChunkerConfig {
            min_size: 0,
            avg_size: 0,
            max_size: 0,
            whole_file_threshold: ChunkerConfig::DEFAULT_WHOLE_FILE_THRESHOLD,
        };
        let err = chunk_to_vec(&b"some bytes"[..], config).await.unwrap_err();
        assert!(matches!(err, Error::InvalidConfig(_)));
    }

    /// `chunk_to_vec`'s future must be `Send` — a prior version routed
    /// through an `impl AsyncFnMut` combinator whose captured-reference
    /// future the compiler couldn't prove `Send` for every lifetime,
    /// which broke exactly this: chunking inside `tokio::spawn`.
    #[tokio::test]
    async fn future_is_send_under_tokio_spawn() {
        let data = vec![1u8; 64 * 1024];
        let handle =
            tokio::spawn(async move { chunk_to_vec(&data[..], ChunkerConfig::default()).await });
        let chunks = handle.await.unwrap().unwrap();
        assert!(!chunks.is_empty());
    }
}
