//! Keyflow chart → inline SVG. Public API mirrors
//! [`editor_mermaid::render_svg`] and `editor_typst::Compiler`
//! so the markdown integration layer treats all three fence
//! renderers the same way: one `render_svg(source)` →
//! `Result<String, RenderError>` call, no setup, no state.
//!
//! Under the hood we drive keyflow's `engraver` through its
//! **CPU-only `svg` tier** — the layout engine plus the
//! `SvgSerializer`, with the wgpu/vello GPU stack and the PDF
//! deps compiled out. That subset is pure-Rust
//! (kurbo/peniko/skrifa/smufl) and compiles for
//! `wasm32-unknown-unknown`, so `.kf` fences render fully in
//! the browser like typst and mermaid.
//!
//! ## Self-contained output
//!
//! Music engraving leans on specific fonts (Leland/Bravura for
//! SMuFL glyphs, MuseJazz/Chicago for text). We embed those
//! font bytes into the SVG as base64 `@font-face` blocks
//! (`with_embedded_font`), so the emitted SVG renders correctly
//! in any browser without the fonts being installed. The bytes
//! themselves are `include_bytes!`-baked into `engraver`, so no
//! filesystem access is needed at runtime.

use engraver::api;
use engraver::export::svg::{SvgExportConfig, SvgSerializer};
use engraver::fonts::ChartFontBundle;
use engraver::layout::chart::LayoutMode;
use thiserror::Error;

/// Layout width (points) for the continuous-scroll render. The
/// emitted SVG carries this as its intrinsic width; the editor
/// scales it to the container with CSS, so this only sets how
/// wide the engraver lets a system grow before wrapping.
const LAYOUT_WIDTH: f64 = 800.0;

/// Padding (points) kept around the content when cropping a snippet's viewBox
/// to its bounds — a little breathing room so glyph edges and chord symbols
/// above the staff aren't flush against the embed border.
const SNIPPET_PADDING: f64 = 6.0;

#[derive(Debug, Error)]
pub enum RenderError {
    /// The keyflow source didn't parse, or layout/serialization
    /// failed. Body carries the human-readable error. The caller
    /// typically falls back to showing the raw source so the
    /// user can fix it.
    #[error("keyflow render failed: {0}")]
    Render(String),
}

/// Render keyflow chart source to an inline SVG string. The
/// result is safe to drop into `dangerous_inner_html` — it
/// begins with `<svg …>` and is **self-contained** (fonts embedded
/// as base64 `@font-face`), so it renders anywhere with no setup.
///
/// That self-containment costs ~4 MB per SVG and ~10 ms to build.
/// For a one-off embed (a markdown `.kf` fence) that's fine. For a
/// surface that re-renders on every keystroke, prefer
/// [`render_svg_live`] + [`font_face_css`], which moves the font
/// bytes out of the hot path.
///
/// Uses continuous-scroll layout (a single scene-sized image,
/// no pagination) which is the right shape for an inline
/// document embed. Returns [`RenderError::Render`] when the
/// source can't be parsed or laid out.
pub fn render_svg(source: &str) -> Result<String, RenderError> {
    render_inner(source, true)
}

/// Render keyflow chart source to an inline SVG **without** embedded
/// fonts — glyphs only, ~38 KB instead of ~4 MB, ~6 ms instead of
/// ~16 ms (native; the gap is wider in wasm and includes the DOM
/// cost of inserting/parsing 4 MB of base64 each time).
///
/// The output references its fonts by family name, so the host must
/// have those `@font-face` rules in the document — inject
/// [`font_face_css`] **once** at page level. Inline `<svg>` resolves
/// document fonts by name, so each live render carries only the
/// changing glyphs. Use this for live editors / previews that
/// re-render as the user types.
pub fn render_svg_live(source: &str) -> Result<String, RenderError> {
    render_inner(source, false)
}

/// Syntax-highlight keyflow source into HTML — one `<span class="kf-…"
/// style="color:…">` per token, unstyled gaps HTML-escaped, newlines
/// preserved. Uses the same per-line highlighter the live editor runs
/// (`keyflow_text::highlighting`), so a source-visible markdown fence
/// reads with the exact colors of the editor. Theme colors are baked
/// inline, so the output is self-contained (no external stylesheet).
///
/// Returns the plain HTML-escaped source when nothing highlights.
pub fn highlight_html(source: &str) -> String {
    use keyflow_text::highlighting::{HighlightSpan, Highlighter, Renderer, Theme};

    // Highlight per line (the highlighter is line-oriented), shifting
    // each line's span offsets to absolute positions in `source` —
    // mirrors editor-keyflow-lang's decoration pass.
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut line_start = 0usize;
    for line in source.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        for span in Highlighter::highlight_line(content) {
            spans.push(HighlightSpan::from_range(
                line_start + span.span.start,
                span.span.len,
                span.kind,
            ));
        }
        line_start += line.len();
    }

    Renderer::to_html_inline(source, &spans, &Theme::default_dark())
}

/// The `@font-face` CSS for the engraving fonts ([`render_svg_live`]
/// references these families by name). Inject the returned string
/// once into the document (e.g. a `<style>` tag); it's ~4 MB of
/// base64 but the browser parses and caches it a single time, versus
/// re-embedding it in every keystroke's SVG.
pub fn font_face_css() -> Result<String, RenderError> {
    let fonts = ChartFontBundle::new().map_err(RenderError::Render)?;
    Ok(with_embedded_fonts(&fonts, SvgExportConfig::default()).font_face_css())
}

/// Shared layout + crop + serialize. `embed_fonts` toggles between
/// the self-contained [`render_svg`] and the font-less
/// [`render_svg_live`].
fn render_inner(source: &str, embed_fonts: bool) -> Result<String, RenderError> {
    // Empty (or whitespace-only) input isn't an error here — it's a chart that
    // hasn't been written yet. `parse_chart` rejects it with "Empty input", but
    // for a live editor that should read as "nothing yet", not a red error box.
    // Render to an empty string so the preview pane is simply blank.
    if source.trim().is_empty() {
        return Ok(String::new());
    }

    let mode = LayoutMode::ContinuousScroll {
        width: LAYOUT_WIDTH,
    };
    let result =
        api::chart::layout_text(source, &mode).map_err(|e| RenderError::Render(e.to_string()))?;

    // Shrink-wrap the SVG viewBox to what's actually drawn. The engraver's
    // `total_width`/`total_height` describe a print page box — content plus A4
    // margins, inter-system spacing, and below-staff reserve — which for an
    // inline snippet leaves the music marooned in mostly-empty space (a
    // one-system chart is ~20pt of music in a ~180pt box). Cropping to the
    // content bounds (with a little padding) makes the embed size to its
    // content; the editor's CSS then scales that to the container.
    let (vx, vy, vw, vh) = match result.content_bounds() {
        Some(b) => (
            b.x0 - SNIPPET_PADDING,
            b.y0 - SNIPPET_PADDING,
            b.width() + 2.0 * SNIPPET_PADDING,
            b.height() + 2.0 * SNIPPET_PADDING,
        ),
        None => (0.0, 0.0, result.total_width, result.total_height),
    };
    let base = SvgExportConfig::for_page(vx, vy, vw, vh);
    let config = if embed_fonts {
        let fonts = ChartFontBundle::new().map_err(RenderError::Render)?;
        with_embedded_fonts(&fonts, base)
    } else {
        base
    };
    let mut serializer = SvgSerializer::new(config);
    Ok(serializer.serialize(&result.scene))
}

/// Embed the engraving fonts into the SVG config. Family names
/// mirror keyflow-cli's exporter so every `font-family` the
/// scene references — SMuFL music glyphs, chord-symbol text, and
/// document text, plus their legacy aliases — resolves to baked
/// bytes.
fn with_embedded_fonts(fonts: &ChartFontBundle, config: SvgExportConfig) -> SvgExportConfig {
    let leland = fonts.symbol_font_data().as_ref().clone();
    let leland_text = fonts.leland_text_font_data().as_ref().clone();
    let musejazz_text = fonts.text_font_data().as_ref().clone();
    let musejazz = fonts.musejazz_font_data().as_ref().clone();
    let chicago = fonts.chicago_font_data().as_ref().clone();
    let bravura = fonts.bravura_font_data().as_ref().clone();
    let freesans = fonts.freesans_font_data().as_ref().clone();

    config
        // SMuFL music font (Leland) + legacy "Bravura" alias.
        .with_embedded_font("Leland", leland)
        .with_embedded_font("Bravura", bravura)
        // Leland Text companion + aliases.
        .with_embedded_font("Leland Text", leland_text.clone())
        .with_embedded_font("LelandText", leland_text.clone())
        .with_embedded_font("Edwin", leland_text)
        // MuseJazz music + MuseJazz Text chord-symbol font.
        .with_embedded_font("MuseJazz", musejazz)
        .with_embedded_font("MuseJazz Text", musejazz_text.clone())
        .with_embedded_font("MuseJazzText", musejazz_text)
        // Chicago — default document / title text.
        .with_embedded_font("Chicago", chicago.clone())
        .with_embedded_font("ChicagoFLF", chicago.clone())
        .with_embedded_font("FreeSans", freesans)
        .with_embedded_font("sans-serif", chicago)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "Test Chart - Demo\n4/4 120bpm #C\n\nVS\nC | F | G | C\n";

    #[test]
    fn renders_a_simple_chart() {
        let svg = render_svg(SIMPLE).expect("render ok");
        assert!(svg.contains("<svg"), "got: {}", &svg[..svg.len().min(80)]);
    }

    #[test]
    fn embeds_fonts_for_self_contained_output() {
        let svg = render_svg(SIMPLE).expect("render ok");
        assert!(svg.contains("@font-face"), "expected embedded fonts");
    }

    /// The live render must NOT embed fonts: same drawing, a fraction of the
    /// bytes (glyphs only). Fonts come from `font_face_css()` injected once. This
    /// is what keeps the per-keystroke preview cheap.
    #[test]
    fn live_render_is_font_less_and_small() {
        let live = render_svg_live(SIMPLE).expect("render ok");
        let full = render_svg(SIMPLE).expect("render ok");
        assert!(live.contains("<svg"), "live render should be an svg");
        assert!(
            !live.contains("@font-face"),
            "live render must not embed fonts"
        );
        assert!(
            live.len() * 10 < full.len(),
            "live render should be far smaller than the embedded one (live={}KB full={}KB)",
            live.len() / 1024,
            full.len() / 1024
        );
    }

    /// Empty / whitespace-only input is "nothing yet", not an error: both
    /// render entry points return an empty string so a live preview is blank
    /// instead of showing keyflow's "Empty input" parse error.
    #[test]
    fn empty_input_renders_nothing_not_an_error() {
        for src in ["", "   ", "\n\n", "\t \n  "] {
            assert_eq!(
                render_svg_live(src).expect("empty input should not error"),
                "",
                "live render of {src:?} should be empty"
            );
            assert_eq!(
                render_svg(src).expect("empty input should not error"),
                "",
                "render of {src:?} should be empty"
            );
        }
        // A real chart still renders.
        assert!(render_svg_live("1 4 6 5").expect("ok").contains("<svg"));
    }

    /// The page injects this once so font-less live renders resolve their
    /// families. It must carry the @font-face rules and the families the scene
    /// references by name.
    #[test]
    fn font_face_css_carries_the_engraving_families() {
        let css = font_face_css().expect("font css");
        assert!(css.contains("@font-face"), "expected @font-face rules");
        for family in ["Leland", "MuseJazz Text", "Chicago"] {
            assert!(
                css.contains(&format!("font-family: '{family}'")),
                "missing @font-face for {family}"
            );
        }
    }

    /// A one-system chart is a few measures of music; its SVG must crop to that,
    /// not to the engraver's print page box (which pads it with margins +
    /// trailing system spacing into ~5× the height). Guards the snippet crop.
    #[test]
    fn crops_to_content_not_page_box() {
        let svg = render_svg("1 4 6 5").expect("render ok");
        let head: String = svg.chars().take(400).collect();
        let attr = |name: &str| -> f64 {
            head.split(&format!("{name}=\""))
                .nth(1)
                .and_then(|s| s.split('"').next())
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(f64::NAN)
        };
        let h = attr("height");
        // Four bars of single-line chord/staff content crop to well under 80pt;
        // the un-cropped page box was ~178pt. A regression (reverting to the
        // page box) would blow past this.
        assert!(
            h < 80.0,
            "snippet height should crop to content (~33pt), got {h}"
        );
    }

    #[test]
    fn highlight_html_wraps_tokens_and_preserves_source() {
        let html = highlight_html("Cmaj7 | F#m7b5 | Bbmaj9 | G7b9");
        // Roots/qualities/extensions each get their own kf-* span
        // (a chord is split per token, not wrapped whole).
        assert!(
            html.contains("class=\"kf-root\""),
            "root token span: {html}"
        );
        assert!(html.contains("class=\"kf-quality\""), "quality token span");
        assert!(
            html.contains("class=\"kf-extension\""),
            "extension token span"
        );
        // Every character of the source survives (nothing dropped):
        // stripping tags leaves the original text.
        let stripped = strip_tags(&html);
        assert_eq!(stripped, "Cmaj7 | F#m7b5 | Bbmaj9 | G7b9");
    }

    #[test]
    fn highlight_html_escapes_and_keeps_newlines() {
        let html = highlight_html("Am7\nDm7");
        assert!(html.contains('\n'), "newline between lines preserved");
        assert_eq!(strip_tags(&html), "Am7\nDm7");
    }

    /// Remove `<span …>`/`</span>` tags and unescape the handful of
    /// entities the renderer emits, recovering the original source.
    fn strip_tags(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
    }
}

/// The chart renderer, as an `editor-state` fence renderer.
///
/// `editor-state` cannot depend on this crate — it would put the whole
/// editor stack above the notation domain and stop the editor being
/// embeddable on its own — so the dependency is inverted through a
/// registry. Register once at application start:
///
/// ```ignore
/// editor_state::fence_renderer::register_fence_renderer(
///     "kf",
///     std::sync::Arc::new(editor_keyflow::Fences),
/// );
/// ```
pub struct Fences;

impl editor_state::fence_renderer::FenceRenderer for Fences {
    fn render_svg(&self, source: &str) -> Option<String> {
        match render_svg(source) {
            Ok(svg) => Some(svg),
            Err(e) => {
                tracing::debug!(?e, len = source.len(), "keyflow fence render failed");
                None
            }
        }
    }

    fn highlight_html(&self, source: &str) -> String {
        highlight_html(source)
    }
}
