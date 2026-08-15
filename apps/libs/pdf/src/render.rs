//! Generic render entry points. Domain-specific helpers
//! (e.g. [`crate::render_invoice`]) build on top of these.

use crate::error::PdfError;

/// Render a self-contained HTML string into PDF bytes.
/// Embed CSS inline via `<style>...</style>` or
/// `style="..."` attributes. Fulgur fetches no external
/// resources by default — every asset must be inline or
/// embedded as a `data:` URL.
pub fn render_html(html: &str) -> Result<Vec<u8>, PdfError> {
    let engine = fulgur::engine::Engine::builder().build();
    engine
        .render_html(html)
        .map_err(|e| PdfError::Fulgur(e.to_string()))
}

/// Render a MiniJinja template + JSON data. The template
/// name is the cache key fulgur shows in error messages; it
/// can be anything sensible (e.g. `"invoice.html"`).
///
/// `data` is serialized to `serde_json::Value` so the
/// caller can pass either a `&serde_json::Value` directly
/// or any `Serialize` struct via `serde_json::to_value`.
pub fn render_template<T: serde::Serialize>(
    template_name: &str,
    template_body: &str,
    data: &T,
) -> Result<Vec<u8>, PdfError> {
    let json = serde_json::to_value(data)?;
    let engine = fulgur::engine::Engine::builder()
        .template(template_name, template_body)
        .data(json)
        .build();
    engine
        .render()
        .map_err(|e| PdfError::Fulgur(e.to_string()))
}

/// `true` if the byte slice starts with the standard PDF
/// magic number. Cheap smoke check after a render.
#[must_use]
pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}
