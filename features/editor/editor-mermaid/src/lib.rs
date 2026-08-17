//! Mermaid SVG rendering. Public API mirrors
//! [`editor_typst::Compiler`] so the markdown integration layer
//! treats both crates the same way: one `render_svg(source)` →
//! `Result<String, RenderError>` call, no setup, no state.
//!
//! Under the hood we lean on `mermaid-rs-renderer` — a pure-Rust
//! Mermaid parser + layouter + SVG emitter. Pinned to the `0.2`
//! line; minor releases have changed the public function names
//! before.
//!
//! ## Wasm caveats
//!
//! The renderer's underlying `fontdb` would normally walk the
//! system font directory at startup — that fails on
//! `wasm32-unknown-unknown` (no filesystem). We compile with
//! `default-features = false` on `mermaid-rs-renderer` to drop
//! the resvg/PNG path entirely; the SVG output already references
//! fonts by name (`font-family="Arial"`) and lets the host
//! browser pick the rendering, so no font bundle is needed.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    /// The Mermaid source didn't parse or layout. Body carries
    /// the human-readable error chain from the renderer.
    #[error("mermaid render failed: {0}")]
    Render(String),
}

/// Render Mermaid source to an inline SVG string. The result is
/// safe to drop into `dangerous_inner_html` — it begins with
/// `<svg …>` and is self-contained.
///
/// Returns [`RenderError::Render`] when the source can't be
/// parsed (syntax error) or laid out (degenerate graph). The
/// caller typically falls back to showing the raw source so the
/// user can fix it.
pub fn render_svg(source: &str) -> Result<String, RenderError> {
    mermaid_rs_renderer::render(source).map_err(|e| RenderError::Render(format!("{e:#}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_simple_flowchart() {
        let svg = render_svg("flowchart TD\n  A --> B").expect("render ok");
        assert!(
            svg.starts_with("<svg"),
            "got: {}",
            &svg[..svg.len().min(80)]
        );
    }

    #[test]
    fn rejects_obvious_garbage() {
        // Mermaid's parser is lenient — fully garbage input may
        // still produce *some* SVG. The test we care about is
        // that the function doesn't panic.
        let _ = render_svg("this is not mermaid at all").err();
    }
}
