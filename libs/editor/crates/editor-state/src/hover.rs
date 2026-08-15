//! Hover tooltips — content shown when the pointer rests over a position.
//!
//! Mirrors `@codemirror/view`'s `hoverTooltip` (defined in
//! `~/Development/research/codemirror/view/src/tooltip.ts`), but the *source*
//! is pure data so it lives in the state core: a [`HoverSource`] maps the
//! current document + a byte offset to an optional [`HoverTooltip`]. The view
//! layer (`editor-view`) owns the pointer tracking, positioning, and DOM; this
//! crate only describes *what* to show.
//!
//! A language integration (e.g. `editor-keyflow-lang`) supplies a `HoverSource`
//! the same way it supplies a `DecorationSource`.

use crate::state::EditorState;

/// What to render in a hover tooltip, plus the document range it describes.
///
/// `html` is injected verbatim into the floating tooltip element (via
/// `dangerous_inner_html`), so the producer is responsible for escaping any
/// untrusted text. `from`/`to` are byte offsets used to anchor the tooltip to
/// the hovered token (and to decide when the pointer has left it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverTooltip {
    pub html: String,
    pub from: usize,
    pub to: usize,
}

impl HoverTooltip {
    pub fn new(range: std::ops::Range<usize>, html: impl Into<String>) -> Self {
        Self {
            from: range.start,
            to: range.end,
            html: html.into(),
        }
    }
}

/// A hover source: given the editor state and the byte offset under the
/// pointer, return a tooltip to show (or `None` for nothing here).
///
/// A plain `fn` pointer (no captured state), matching
/// [`crate::decoration`]'s `DecorationSource` shape — within a single binary
/// `fn`-ptr equality lets the view cheaply detect when the source changed.
pub type HoverSource = fn(&EditorState, usize) -> Option<HoverTooltip>;
