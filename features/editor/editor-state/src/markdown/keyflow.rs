//! Keyflow chart rendering for ```` ```kf ``` ```` fences. Mirror of
//! the `mermaid` submodule — same per-pass compile budget + LRU
//! cache shape, talking to `editor-keyflow` instead.
//!
//! Each cache entry is `(source) → svg`. Unlike mermaid we don't
//! recolor the output: a chart is engraved on "paper" (the SVG
//! carries its own light background and ink colors), so it reads
//! the same in light or dark editor themes.

use std::cell::Cell;

const CACHE_CAP: usize = 64;
/// Conservative: a chart layout pass (parse + engrave + serialize)
/// is heavier than a Typst math fragment, so we allow one cold
/// render per `live_preview` pass and rely on the cache otherwise.
const COMPILE_BUDGET_PER_PASS: u8 = 1;

thread_local! {
    static COMPILE_BUDGET: Cell<u8> = const { Cell::new(COMPILE_BUDGET_PER_PASS) };
}

/// Re-arm the per-pass budget. Call at the top of every
/// `live_preview` pass.
pub(crate) fn reset_compile_budget() {
    COMPILE_BUDGET.with(|c| c.set(COMPILE_BUDGET_PER_PASS));
}

/// Render keyflow chart source to SVG. Returns `None` on a cache
/// miss when the budget is exhausted (caller falls back to source)
/// or when the source can't be parsed / laid out.
pub(crate) fn render_keyflow(body: &str) -> Option<String> {
    if let Some(cached) = with_keyflow_cache(|c| c.get(body)) {
        return Some(cached);
    }
    let budget = COMPILE_BUDGET.with(std::cell::Cell::get);
    if budget == 0 {
        return None;
    }
    COMPILE_BUDGET.with(|c| c.set(budget - 1));

    // Resolved through the fence registry rather than a direct dependency:
    // the chart renderer lives above this crate (it needs `keyflow-text`
    // and `engraver`), so depending on it here would drag the whole editor
    // stack above the notation domain. See `crate::fence_renderer`.
    //
    // Nothing registered means charts render as source, which is the same
    // fallback any unknown fence language gets.
    let renderer = crate::fence_renderer::fence_renderer(LANGUAGE)?;
    match renderer.render_svg(body) {
        Some(svg) => {
            with_keyflow_cache(|c| c.put(body.to_string(), svg.clone()));
            Some(svg)
        }
        None => {
            tracing::debug!(body_len = body.len(), "keyflow render declined");
            None
        }
    }
}

/// Registry key for the keyflow fence family (`kf`, `kf+`, `kf-`).
pub(crate) const LANGUAGE: &str = "kf";

/// Syntax-highlight a keyflow fence body, falling back to escaped source
/// when no chart renderer is registered.
pub(crate) fn highlight_keyflow(body: &str) -> String {
    match crate::fence_renderer::fence_renderer(LANGUAGE) {
        Some(r) => r.highlight_html(body),
        None => super::escape_html(body),
    }
}

struct KeyflowCache {
    entries: Vec<(String, String)>,
    cap: usize,
}

impl KeyflowCache {
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

fn with_keyflow_cache<R>(f: impl FnOnce(&mut KeyflowCache) -> R) -> R {
    thread_local! {
        static CACHE: std::cell::RefCell<KeyflowCache> =
            std::cell::RefCell::new(KeyflowCache::new(CACHE_CAP));
    }
    CACHE.with(|c| f(&mut c.borrow_mut()))
}
