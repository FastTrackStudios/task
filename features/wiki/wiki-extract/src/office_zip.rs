//! PPTX / DOCX (and other zipped `OpenXML`) image extraction.
//! Walks the archive for `ppt/media/`, `word/media/`, or
//! `xl/media/` entries; decodes each with `image` to read
//! dimensions for the size filter.

use std::io::Read;

use image::ImageReader;
use wiki_proto::multimodal::{ExtractOpts, ExtractedImage};
use zip::ZipArchive;

use crate::ExtractError;
use crate::images::sha256_hex;

const MEDIA_PREFIXES: &[&str] = &[
    "ppt/media/",
    "word/media/",
    "xl/media/",
    "ppt/diagrams/",
    "word/embeddings/",
];

pub(crate) fn extract_office(
    bytes: &[u8],
    opts: &ExtractOpts,
) -> Result<Vec<ExtractedImage>, ExtractError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut zip = ZipArchive::new(cursor).map_err(|e| ExtractError::Decode(e.to_string()))?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut entry = match zip.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().to_string();
        if !MEDIA_PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        if let Some(skip) = opts
            .exclude_substrings
            .iter()
            .find(|s| name.contains(s.as_str()))
        {
            let _ = skip;
            continue;
        }
        let mut bytes_buf = Vec::with_capacity(entry.size() as usize);
        if entry.read_to_end(&mut bytes_buf).is_err() {
            continue;
        }
        let mime = guess_mime(&name);
        if !mime.starts_with("image/") {
            continue;
        }
        let reader = match ImageReader::new(std::io::Cursor::new(&bytes_buf)).with_guessed_format()
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        let dim = match reader.into_dimensions() {
            Ok(d) => d,
            Err(_) => continue,
        };
        if dim.0 < opts.min_width || dim.1 < opts.min_height {
            continue;
        }
        out.push(ExtractedImage {
            index: out.len() as u32,
            mime,
            page: 0,
            width: dim.0,
            height: dim.1,
            sha256: sha256_hex(&bytes_buf),
            bytes: bytes_buf,
        });
        if opts.max_images > 0 && out.len() as u32 >= opts.max_images {
            break;
        }
    }
    Ok(out)
}

// `lower` is already lowercased so case-sensitive comparisons here are
// correct — silence the lint that can't see the prior `.to_ascii_lowercase()`.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn guess_mime(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png".into()
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".into()
    } else if lower.ends_with(".gif") {
        "image/gif".into()
    } else if lower.ends_with(".webp") {
        "image/webp".into()
    } else if lower.ends_with(".bmp") {
        "image/bmp".into()
    } else if lower.ends_with(".tiff") {
        "image/tiff".into()
    } else if lower.ends_with(".svg") {
        "image/svg+xml".into()
    } else {
        "application/octet-stream".into()
    }
}
