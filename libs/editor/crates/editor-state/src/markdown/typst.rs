//! Typst SVG rendering for the live-preview decorations.
//! Three kinds of fragment:
//!
//! - inline math `$x$` → small bare SVG
//! - block math `$$x$$` → display-mode SVG
//! - ```` ```typst ``` ```` fence body → full Typst doc SVG
//!
//! Compiles synchronously via `editor-typst::Compiler` but
//! defends against worst-case latency two ways:
//!
//! 1. **Per-pass compile budget.** `live_preview` calls
//!    [`reset_compile_budget`] at the start of each render
//!    pass. Cache misses count against the budget — once it's
//!    exhausted, further misses return `None` and the caller
//!    falls back to showing the source. The next live-preview
//!    pass picks up where this one left off (the cache is
//!    persistent across passes), so a doc with N fresh
//!    equations converges over ~⌈N/budget⌉ render cycles
//!    instead of blocking once for ~N×50ms.
//!
//! 2. **Thread-local LRU cache** keyed by `(kind, body)`. Cap
//!    is generous (128) so popular fragments stay hot through
//!    a long editing session — typing into a paragraph nearby
//!    doesn't evict the equation above.
//!
//! The SVG is post-processed to swap our sentinel fill color
//! (`#ff00fe`) for `currentColor`, so the rendered glyphs
//! inherit the editor pane's CSS `color:` and respond to theme
//! switches without a recompile.

use std::cell::Cell;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TypstKind {
    MathInline,
    MathBlock,
    /// ```` ```typst …``` ```` fence body, compiled as a full
    /// Typst document.
    Block,
}

const SENTINEL: &str = "#ff00fe";
const CACHE_CAP: usize = 128;
/// How many cold compiles we allow per `live_preview` pass.
/// Picked to keep the worst-case at ~2×50ms = ~100ms; tune if
/// profiling shows otherwise.
const COMPILE_BUDGET_PER_PASS: u8 = 2;

thread_local! {
    static COMPILE_BUDGET: Cell<u8> = const { Cell::new(COMPILE_BUDGET_PER_PASS) };
}

/// Re-arm the per-pass compile budget. Call once at the top of
/// every `live_preview` pass before any [`render_typst`] calls.
pub(crate) fn reset_compile_budget() {
    COMPILE_BUDGET.with(|c| c.set(COMPILE_BUDGET_PER_PASS));
}

/// Render a Typst source fragment to inline SVG. Returns `None`
/// when (a) compile fails, or (b) the per-pass budget is
/// exhausted and the body isn't cached. The caller shows the
/// raw source in either case — the user can keep editing.
pub(crate) fn render_typst(kind: TypstKind, body: &str) -> Option<String> {
    if let Some(cached) = with_typst_cache(|c| c.get(kind, body)) {
        return Some(cached);
    }
    let budget = COMPILE_BUDGET.with(std::cell::Cell::get);
    if budget == 0 {
        return None;
    }
    COMPILE_BUDGET.with(|c| c.set(budget - 1));

    // Wrap the fragment in a Typst preamble so each compiles
    // as a standalone document. `page(fill: none)` keeps the
    // SVG background transparent; the sentinel fill color is
    // replaced with `currentColor` after compile.
    let prelude = format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(size: 14pt, fill: rgb(\"{SENTINEL}\"))\n"
    );
    let wrapped = match kind {
        TypstKind::MathInline => format!("{prelude}${body}$"),
        TypstKind::MathBlock => format!("{prelude}$ {body} $"),
        TypstKind::Block => format!("{prelude}{body}"),
    };
    let mut compiler = editor_typst::Compiler::new();
    compiler.set_source(wrapped);
    match compiler.compile_svg() {
        Ok(svg) => {
            // Typst emits hex literals lowercase but be
            // defensive about an uppercase variant if it ever
            // changes.
            let themed = svg
                .replace("#ff00fe", "currentColor")
                .replace("#FF00FE", "currentColor");
            with_typst_cache(|c| c.put(kind, body.to_string(), themed.clone()));
            Some(themed)
        }
        Err(e) => {
            tracing::debug!(?e, body_len = body.len(), "typst compile failed");
            None
        }
    }
}

struct TypstCache {
    entries: Vec<(TypstKind, String, String)>,
    cap: usize,
}

impl TypstCache {
    fn new(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
            cap,
        }
    }
    fn get(&mut self, kind: TypstKind, body: &str) -> Option<String> {
        let i = self
            .entries
            .iter()
            .position(|(k, b, _)| *k == kind && b == body)?;
        let hit = self.entries.remove(i);
        let svg = hit.2.clone();
        self.entries.push(hit);
        Some(svg)
    }
    fn put(&mut self, kind: TypstKind, body: String, svg: String) {
        if self.entries.len() >= self.cap {
            self.entries.remove(0);
        }
        self.entries.push((kind, body, svg));
    }
}

fn with_typst_cache<R>(f: impl FnOnce(&mut TypstCache) -> R) -> R {
    thread_local! {
        static CACHE: std::cell::RefCell<TypstCache> =
            std::cell::RefCell::new(TypstCache::new(CACHE_CAP));
    }
    CACHE.with(|c| f(&mut c.borrow_mut()))
}
