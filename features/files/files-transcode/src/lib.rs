//! Files transcode pipeline (issue #269): server-side derived media —
//! H.264 proxies, an AAC audio rendition, waveform peaks, filmstrip
//! thumbnails — produced from source media and cached in the CAS,
//! keyed by `(source FileId, recipe version, kind)`, GC-tied to their
//! source.
//!
//! - [`recipe`] — the rendition kinds, the ladder each media class
//!   yields, and the recipe version that keys the cache.
//! - [`transcoder::Transcoder`] — the ffmpeg driver, behind a trait so
//!   the caching / GC / recipe logic is testable without ffmpeg; the
//!   real driver is behind the `ffmpeg` feature.
//! - [`store::RenditionStore`] — the CAS-backed rendition cache with the
//!   two GC rules (source-tied + recipe-current).
//! - [`pipeline::TranscodePipeline`] — the lazy generate-and-cache
//!   engine, plus the checkpoint-trigger warm-up.
//!
//! The wire surface (`TranscodeService` — request a rendition, warm a
//! checkpoint) lives in the sibling `files` crate's server, which owns
//! the version store and the checkpoint hook this pipeline hangs off.

pub mod error;
pub mod pipeline;
pub mod recipe;
pub mod store;
pub mod transcoder;

#[cfg(test)]
mod tests;

pub use error::{Error, Result};
pub use pipeline::{Rendition, TranscodePipeline};
pub use recipe::{MediaClass, RECIPE_VERSION, RenditionKind};
pub use store::{RenditionKey, RenditionStore};
pub use transcoder::Transcoder;
