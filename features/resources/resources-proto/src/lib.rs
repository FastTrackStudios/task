//! Wasm-clean wire contract for the Resource Library.
//!
//! The transcript / annotation sidecars live on the server under
//! `<org>/resources/` (a native tier, not the vault), so the in-app
//! watch + reader views read them over this `#[architect::rpc]` service
//! rather than fetching files. Mirrors the `links-proto` shape.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One transcript cue — spoken text and when it occurs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct TranscriptSegment {
    /// Start time in seconds.
    pub start: f32,
    /// Duration in seconds.
    #[serde(default)]
    pub dur: f32,
    pub text: String,
}

/// A resource's full transcript (matches the `<slug>.transcript.json`
/// sidecar shape, so the backend deserializes straight into it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct TranscriptDoc {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub source: String,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum ResourcesError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(String),
}

#[architect::rpc]
pub trait ResourcesService {
    /// Read a transcript sidecar at `<org>/resources/<rel_path>`
    /// (e.g. `sermons/god-restores-broken-people.transcript.json`).
    /// Path traversal (`..`) is rejected.
    fn transcript(&self, rel_path: &str) -> Result<TranscriptDoc, ResourcesError>;
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{TranscriptDoc, TranscriptSegment};
    unsafe impl vox_types::Reborrow for TranscriptSegment {
        type Ref<'a> = TranscriptSegment;
    }
    unsafe impl vox_types::Reborrow for TranscriptDoc {
        type Ref<'a> = TranscriptDoc;
    }
}
