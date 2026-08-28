//! Pluggable renderers for fenced code blocks.
//!
//! Most fence languages the editor understands are compiled in:
//! `editor-typst` and `editor-mermaid` are ordinary dependencies, because
//! they sit at the same layer as the editor and travel with it.
//!
//! Keyflow does not. The chart language lives in its own repo *above* this
//! one — `editor-keyflow` needs `keyflow-text` and `engraver` — so a direct
//! dependency here would make the whole editor stack sit above the notation
//! domain and stop it being embeddable on its own. This registry is the
//! seam: `editor-state` declares what it needs from a chart renderer, and
//! whoever assembles the application plugs one in.
//!
//! # Wiring
//!
//! ```ignore
//! // Once, at application start — before any document is rendered.
//! editor_state::register_fence_renderer("kf", Arc::new(editor_keyflow::Fences));
//! ```
//!
//! Nothing registered is not an error: an unrenderable fence falls back to
//! showing its source, which is what the editor does for any language it
//! does not know.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Renders one fence language into the editor's live preview.
///
/// Implementations are shared across threads and called on the render
/// path, so they must be cheap on a cache hit and must not block.
pub trait FenceRenderer: Send + Sync {
    /// Render the fence body to a standalone SVG document.
    ///
    /// `None` means "cannot render this" — a parse failure, or a body that
    /// is a syntax illustration rather than a real document. The caller
    /// falls back to displaying the source, so returning `None` is a normal
    /// outcome and not an error path.
    fn render_svg(&self, source: &str) -> Option<String>;

    /// Syntax-highlight the fence body as HTML.
    ///
    /// Must return HTML-safe output: the result is inserted into the
    /// preview without further escaping. Implementations that cannot
    /// highlight should escape the source and return that.
    fn highlight_html(&self, source: &str) -> String;
}

/// The process-wide registry.
///
/// A registry rather than a value threaded through `EditorState` because
/// fence rendering happens deep inside the markdown pass, which is called
/// from render paths that have no business carrying a renderer table. The
/// set of languages is fixed at application start and never varies per
/// document.
fn registry() -> &'static RwLock<HashMap<String, Arc<dyn FenceRenderer>>> {
    static REGISTRY: std::sync::OnceLock<RwLock<HashMap<String, Arc<dyn FenceRenderer>>>> =
        std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a renderer for `language`, replacing any previous one.
///
/// Call once at application start. `language` is matched
/// case-insensitively against the fence's info string, and is the *base*
/// language — the keyflow family (`kf`, `kf+`, `kf-`) all resolve to `kf`.
pub fn register_fence_renderer(language: &str, renderer: Arc<dyn FenceRenderer>) {
    if let Ok(mut map) = registry().write() {
        map.insert(language.to_ascii_lowercase(), renderer);
    }
}

/// The renderer registered for `language`, if any.
#[must_use]
pub fn fence_renderer(language: &str) -> Option<Arc<dyn FenceRenderer>> {
    registry()
        .read()
        .ok()?
        .get(&language.to_ascii_lowercase())
        .map(Arc::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub;
    impl FenceRenderer for Stub {
        fn render_svg(&self, source: &str) -> Option<String> {
            (source != "no").then(|| format!("<svg>{source}</svg>"))
        }
        fn highlight_html(&self, source: &str) -> String {
            format!("<em>{source}</em>")
        }
    }

    #[test]
    fn an_unregistered_language_is_none_not_a_panic() {
        assert!(fence_renderer("definitely-not-registered").is_none());
    }

    #[test]
    fn registers_and_resolves_case_insensitively() {
        register_fence_renderer("StubLang", Arc::new(Stub));
        assert!(fence_renderer("stublang").is_some());
        assert!(fence_renderer("STUBLANG").is_some());
    }

    #[test]
    fn a_renderer_may_decline_to_render() {
        register_fence_renderer("declining", Arc::new(Stub));
        let r = fence_renderer("declining").unwrap();
        assert_eq!(r.render_svg("yes").as_deref(), Some("<svg>yes</svg>"));
        assert_eq!(r.render_svg("no"), None);
    }

    #[test]
    fn registering_twice_replaces() {
        struct Other;
        impl FenceRenderer for Other {
            fn render_svg(&self, _: &str) -> Option<String> {
                Some("other".into())
            }
            fn highlight_html(&self, _: &str) -> String {
                "other".into()
            }
        }
        register_fence_renderer("replaceme", Arc::new(Stub));
        register_fence_renderer("replaceme", Arc::new(Other));
        assert_eq!(
            fence_renderer("replaceme")
                .unwrap()
                .render_svg("x")
                .as_deref(),
            Some("other")
        );
    }
}
