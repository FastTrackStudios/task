//! Direct image decode — reads dimensions for the size
//! filter and emits one `ExtractedImage`.

use image::ImageReader;
use sha2::{Digest, Sha256};
use wiki_proto::multimodal::{ExtractOpts, ExtractedImage};

use crate::ExtractError;

pub(crate) fn extract_image(
    mime: &str,
    bytes: &[u8],
    opts: &ExtractOpts,
) -> Result<Vec<ExtractedImage>, ExtractError> {
    let cursor = std::io::Cursor::new(bytes);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| ExtractError::Decode(e.to_string()))?;
    let dim = reader
        .into_dimensions()
        .map_err(|e| ExtractError::Decode(e.to_string()))?;
    if dim.0 < opts.min_width || dim.1 < opts.min_height {
        return Ok(Vec::new());
    }
    Ok(vec![ExtractedImage {
        index: 0,
        mime: mime.to_string(),
        page: 0,
        width: dim.0,
        height: dim.1,
        bytes: bytes.to_vec(),
        sha256: sha256_hex(bytes),
    }])
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
