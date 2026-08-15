//! Image extraction from raw sources — Phase 1 of `llm_wiki`'s
//! `multimodal-images.md` plan. **No LLM involved**: this is
//! pure decode + filter (pdfium for PDFs, zip + XML walking
//! for PPTX / DOCX, plain decode for image MIME types).
//!
//! Captioning (Phase 3) is LLM-bound and lives in the
//! `wiki-agent` boundary, not here. The trait method
//! returns raw bytes; the agent crate calls a vision model and
//! writes `Wiki/media/<source-slug>/img-<N>.<ext>` into the
//! wiki via the standard ingest path.

use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct ExtractOpts {
    /// Drop images narrower than this. Default: 100.
    pub min_width: u32,
    /// Drop images shorter than this. Default: 100.
    pub min_height: u32,
    /// Cap on images per source document. Default: 500.
    pub max_images: u32,
    /// Filename prefixes / suffixes to ignore (e.g.
    /// `["icon", "logo"]`). Empty = no exclusion.
    pub exclude_substrings: Vec<String>,
}

impl Default for ExtractOpts {
    fn default() -> Self {
        Self {
            min_width: 100,
            min_height: 100,
            max_images: 500,
            exclude_substrings: Vec::new(),
        }
    }
}

/// One image extracted from a raw source. Bytes carried inline
/// — callers persist them as-is or hand off to a captioner.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ExtractedImage {
    /// Sequential index within the source (0-based).
    pub index: u32,
    /// `image/png`, `image/jpeg`, etc.
    pub mime: String,
    /// PDF page number, if the source was a PDF. `0` for
    /// non-paginated formats.
    pub page: u32,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Encoded image bytes (PNG / JPEG / etc.).
    pub bytes: Vec<u8>,
    /// SHA-256 of `bytes`. Used by the agent's dedup cache so
    /// the same image embedded in two sources gets one caption.
    pub sha256: String,
}
