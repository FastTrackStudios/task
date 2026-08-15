//! PDF image extraction via pdfium-render. Behind the
//! `pdf` feature flag — pulls a native pdfium lib.

use pdfium_render::prelude::*;
use wiki_proto::multimodal::{ExtractOpts, ExtractedImage};

use crate::ExtractError;
use crate::images::sha256_hex;

pub(crate) fn extract_pdf(
    bytes: &[u8],
    opts: &ExtractOpts,
) -> Result<Vec<ExtractedImage>, ExtractError> {
    let pdfium = Pdfium::default();
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| ExtractError::Decode(format!("pdfium load: {e}")))?;

    let mut out: Vec<ExtractedImage> = Vec::new();
    for (page_idx, page) in document.pages().iter().enumerate() {
        for object in page.objects().iter() {
            let image_obj = match object.as_image_object() {
                Some(o) => o,
                None => continue,
            };
            let image = match image_obj.get_raw_image() {
                Ok(i) => i,
                Err(_) => continue,
            };
            let (w, h) = (image.width(), image.height());
            if w < opts.min_width || h < opts.min_height {
                continue;
            }
            // pdfium hands us a raw bitmap; re-encode to PNG
            // so the caption helper has a normal file format.
            let mut buf: Vec<u8> = Vec::new();
            let encoder = image::codecs::png::PngEncoder::new(&mut buf);
            let pixels: Vec<u8> = image.as_bytes().to_vec();
            if let Err(e) = ::image::ImageEncoder::write_image(
                encoder,
                &pixels,
                w,
                h,
                ::image::ExtendedColorType::Rgba8,
            ) {
                tracing::warn!(?e, "pdfium png encode failed");
                continue;
            }
            out.push(ExtractedImage {
                index: out.len() as u32,
                mime: "image/png".into(),
                page: page_idx as u32,
                width: w,
                height: h,
                sha256: sha256_hex(&buf),
                bytes: buf,
            });
            if opts.max_images > 0 && out.len() as u32 >= opts.max_images {
                return Ok(out);
            }
        }
    }
    Ok(out)
}
