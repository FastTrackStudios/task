//! Rendition kinds and the recipe version (issue #269). A **rendition**
//! is derived media — a proxy, an audio rendition, waveform peaks, a
//! filmstrip — produced from a source media file and cached in the CAS,
//! keyed by `(source FileId, recipe version, kind)`. Bumping
//! [`RECIPE_VERSION`] changes that key, so new renditions are generated
//! and the old ones become collectable (never silently orphaned — see
//! the GC in [`crate::store`]).

use serde::{Deserialize, Serialize};

/// The current recipe version. Bump when a rendition's *definition*
/// changes (a codec/ladder/peak-format change) so caches regenerate
/// rather than serving stale derived media. Part of every rendition
/// key.
pub const RECIPE_VERSION: u32 = 1;

/// The class of media a source is, deciding which rendition ladder
/// applies. Probed from the source, not guessed from a file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaClass {
    Video,
    Audio,
    /// Not media we transcode (a project file, an image, text).
    Other,
}

/// One derived rendition kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[cfg_attr(feature = "vox", repr(u8))]
pub enum RenditionKind {
    /// H.264 1080p streaming proxy (video).
    Proxy1080,
    /// H.264 720p streaming proxy (video).
    Proxy720,
    /// AAC audio rendition (video → its audio track, or audio → AAC).
    Audio,
    /// Waveform-peak source: mono s16le PCM (audio, and a video's audio
    /// track), reduced to a peak array by the consumer. Not JSON yet —
    /// the JSON shaping is a later pass that bumps [`RECIPE_VERSION`].
    Peaks,
    /// Filmstrip thumbnail strip (video only).
    Filmstrip,
}

impl RenditionKind {
    /// Stable string tag — part of the rendition key and index filename.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            RenditionKind::Proxy1080 => "proxy-1080",
            RenditionKind::Proxy720 => "proxy-720",
            RenditionKind::Audio => "audio-aac",
            RenditionKind::Peaks => "peaks",
            RenditionKind::Filmstrip => "filmstrip",
        }
    }

    /// Parse a [`RenditionKind::tag`].
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "proxy-1080" => RenditionKind::Proxy1080,
            "proxy-720" => RenditionKind::Proxy720,
            "audio-aac" => RenditionKind::Audio,
            "peaks" => RenditionKind::Peaks,
            "filmstrip" => RenditionKind::Filmstrip,
            _ => return None,
        })
    }

    /// The full rendition ladder for a source of `class` — what a
    /// checkpoint warm-up generates and a source is expected to yield
    /// (spec / AC 1: "a checkpointed video yields the rendition ladder +
    /// filmstrip; audio yields peaks").
    #[must_use]
    pub fn ladder_for(class: MediaClass) -> &'static [RenditionKind] {
        match class {
            MediaClass::Video => &[
                RenditionKind::Proxy1080,
                RenditionKind::Proxy720,
                RenditionKind::Audio,
                RenditionKind::Peaks,
                RenditionKind::Filmstrip,
            ],
            MediaClass::Audio => &[RenditionKind::Audio, RenditionKind::Peaks],
            MediaClass::Other => &[],
        }
    }

    /// The MIME type a served rendition carries.
    #[must_use]
    pub fn mime(self) -> &'static str {
        match self {
            RenditionKind::Proxy1080 | RenditionKind::Proxy720 => "video/mp4",
            RenditionKind::Audio => "audio/mp4",
            // Raw mono s16le PCM the consumer reduces to peaks — not JSON
            // until the shaping pass (see the `Peaks` variant doc).
            RenditionKind::Peaks => "application/octet-stream",
            RenditionKind::Filmstrip => "image/jpeg",
        }
    }
}
