//! Phase-1 image extraction from raw sources.
//!
//! No LLM — pure decode + filter. Returns
//! `wiki_proto::multimodal::ExtractedImage` for each
//! image that passes the `ExtractOpts` size filter.
//!
//! ## Format coverage
//!
//! | Source              | Path |
//! |---------------------|------|
//! | `image/png/jpeg/...` | Read bytes, decode dimensions, emit one image. |
//! | `application/zip` + PPTX / DOCX | Walk archive for `ppt/media/` and `word/media/`. |
//! | `application/pdf`   | `pdf` feature flag; pdfium-render pulls native lib. |
//!
//! Everything else returns an empty list. Captioning is a
//! follow-up that lives in `agent-wiki` (vision LLM
//! turn → text per image).

mod images;
mod office_zip;

#[cfg(feature = "pdf")]
mod pdf;

use std::path::Path;

use thiserror::Error;
use wiki_proto::multimodal::{ExtractOpts, ExtractedImage};

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported mime: {0}")]
    Unsupported(String),
    #[error("decode: {0}")]
    Decode(String),
}

/// Extract images from `bytes` based on the recorded MIME
/// type. Returns at most `opts.max_images` images, each at
/// least `opts.min_width` × `opts.min_height`.
pub fn extract(
    mime: &str,
    bytes: &[u8],
    opts: &ExtractOpts,
) -> Result<Vec<ExtractedImage>, ExtractError> {
    if mime.starts_with("image/") {
        return images::extract_image(mime, bytes, opts);
    }
    if mime.contains("openxml") || mime.contains("officedocument") || mime == "application/zip" {
        return office_zip::extract_office(bytes, opts);
    }
    #[cfg(feature = "pdf")]
    if mime == "application/pdf" {
        return pdf::extract_pdf(bytes, opts);
    }
    Err(ExtractError::Unsupported(mime.to_string()))
}

/// Convenience: read a file from disk and dispatch by MIME
/// inferred from its extension.
pub fn extract_path(path: &Path, opts: &ExtractOpts) -> Result<Vec<ExtractedImage>, ExtractError> {
    let bytes = std::fs::read(path)?;
    let mime = match path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    };
    extract(mime, &bytes, opts)
}
