//! Image extraction from raw sources.

use crate::error::WikiError;
use crate::multimodal::{ExtractOpts, ExtractedImage};

#[architect::rpc]
pub trait Multimodal {
    /// Extract embedded images from a raw source. Pure
    /// decode — no LLM. Captioning lives in the agent
    /// layer.
    fn extract_images(
        &self,
        wiki_id: &str,
        source_path: &str,
        opts: ExtractOpts,
    ) -> Result<Vec<ExtractedImage>, WikiError>;
}
