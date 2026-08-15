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

    // `editor-keyflow` wraps engraver's CPU-only `svg` tier (kurbo/
    // peniko/skrifa — no wgpu, no tokio), which compiles for wasm32
    // too, so `kf` fences engrave everywhere the editor runs.
    match editor_keyflow::render_svg(body) {
        Ok(svg) => {
            with_keyflow_cache(|c| c.put(body.to_string(), svg.clone()));
            Some(svg)
        }
        Err(e) => {
            tracing::debug!(?e, body_len = body.len(), "keyflow render failed");
            None
        }
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
