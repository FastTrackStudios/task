//! Mermaid diagram rendering for ```` ```mermaid ``` ```` fences.
//! Mirror of the `typst` submodule — same per-pass compile
//! budget + LRU cache shape, just talking to `editor-mermaid`
//! instead of `editor-typst`.
//!
//! Each cache entry is `(source) → svg`. We don't key on a kind
//! enum because the Mermaid renderer reads the diagram type
//! (`flowchart`, `sequenceDiagram`, …) from the source itself.

use std::cell::Cell;

const CACHE_CAP: usize = 64;
/// Conservative: Mermaid layouts are heavier than Typst math
/// fragments (graph layout vs. a couple of glyphs), so we
/// allow one cold compile per `live_preview` pass and rely on
/// the cache for everything else.
const COMPILE_BUDGET_PER_PASS: u8 = 1;

thread_local! {
    static COMPILE_BUDGET: Cell<u8> = const { Cell::new(COMPILE_BUDGET_PER_PASS) };
}

/// Re-arm the per-pass budget. Call at the top of every
/// `live_preview` pass.
pub(crate) fn reset_compile_budget() {
    COMPILE_BUDGET.with(|c| c.set(COMPILE_BUDGET_PER_PASS));
}

/// Render a Mermaid source string to SVG. Returns `None` on
/// cache miss when the budget is exhausted (caller falls back
/// to source) or when the renderer rejects the source.
pub(crate) fn render_mermaid(body: &str) -> Option<String> {
    if let Some(cached) = with_mermaid_cache(|c| c.get(body)) {
        return Some(cached);
    }
    let budget = COMPILE_BUDGET.with(std::cell::Cell::get);
    if budget == 0 {
        return None;
    }
    COMPILE_BUDGET.with(|c| c.set(budget - 1));

    match editor_mermaid::render_svg(body) {
        Ok(svg) => {
            let themed = themify_svg(&svg);
            with_mermaid_cache(|c| c.put(body.to_string(), themed.clone()));
            Some(themed)
        }
        Err(e) => {
            tracing::debug!(?e, body_len = body.len(), "mermaid render failed");
            None
        }
    }
}

/// Swap the renderer's "modern" theme palette for values that
/// live on the editor's CSS token system. Mermaid bakes colors
/// in as inline `fill=`/`stroke=` attributes, which beat any
/// stylesheet rule unless we use `!important` — string replace
/// is the simplest robust answer. Mapping below targets the
/// six hex values the default theme actually emits (sampled
/// from a real render; see crate test).
///
/// - text → `currentColor` so it tracks the editor's `color`.
/// - background → `transparent` so the surrounding widget's
///   own background shows through.
/// - node fills → a translucent overlay that reads on either
///   light or dark themes.
/// - strokes (nodes + edges + arrows) → `currentColor` at
///   reduced opacity for hierarchy.
///
/// Users with custom themes can swap to `editor_mermaid::render_svg`
/// directly and skip this step.
fn themify_svg(svg: &str) -> String {
    svg
        // Text fills (`#0F172A` slate-900). Must go first so
        // the `#FFFFFF` background rule below doesn't catch
        // any text accidentally.
        .replace("fill=\"#0F172A\"", "fill=\"currentColor\"")
        // Page background rect. Drop it so the widget div's
        // background is what the user sees.
        .replace("fill=\"#FFFFFF\"", "fill=\"transparent\"")
        // Node fill (slate-50). Subtle overlay.
        .replace("fill=\"#F8FAFC\"", "fill=\"rgba(127, 167, 217, 0.08)\"")
        // Node + edge-label strokes (slate-400).
        .replace(
            "stroke=\"#94A3B8\"",
            "stroke=\"currentColor\" stroke-opacity=\"0.35\"",
        )
        // Arrow heads, edges (slate-500).
        .replace(
            "fill=\"#64748B\"",
            "fill=\"currentColor\" fill-opacity=\"0.55\"",
        )
        .replace(
            "stroke=\"#64748B\"",
            "stroke=\"currentColor\" stroke-opacity=\"0.55\"",
        )
}

struct MermaidCache {
    entries: Vec<(String, String)>,
    cap: usize,
}

impl MermaidCache {
    fn new(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
            cap,
        }
    }
    fn get(&mut self, body: &str) -> Option<String> {
        let i = self.entries.iter().position(|(b, _)| b == body)?;
        let hit = self.entries.remove(i);
        let svg = hit.1.clone();
        self.entries.push(hit);
        Some(svg)
    }
    fn put(&mut self, body: String, svg: String) {
        if self.entries.len() >= self.cap {
            self.entries.remove(0);
        }
        self.entries.push((body, svg));
    }
}

fn with_mermaid_cache<R>(f: impl FnOnce(&mut MermaidCache) -> R) -> R {
    thread_local! {
        static CACHE: std::cell::RefCell<MermaidCache> =
            std::cell::RefCell::new(MermaidCache::new(CACHE_CAP));
    }
    CACHE.with(|c| f(&mut c.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::themify_svg;

    #[test]
    fn themify_swaps_text_for_currentcolor() {
        let svg = r##"<svg><text fill="#0F172A">x</text></svg>"##;
        let out = themify_svg(svg);
        assert!(out.contains("fill=\"currentColor\""));
        assert!(!out.contains("#0F172A"));
    }

    #[test]
    fn themify_drops_white_background() {
        let svg = r##"<svg><rect fill="#FFFFFF"/></svg>"##;
        let out = themify_svg(svg);
        assert!(out.contains("fill=\"transparent\""));
    }

    #[test]
    fn themify_preserves_other_colors() {
        // A user-set custom color shouldn't be touched.
        let svg = r##"<svg><rect fill="#abc123"/></svg>"##;
        let out = themify_svg(svg);
        assert!(out.contains("#abc123"));
    }
}
