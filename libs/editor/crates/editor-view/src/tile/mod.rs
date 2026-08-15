//! The tile tree — a Rust port of `CodeMirror` 6's
//! `view/src/tile.ts`. The tile tree is the editor's
//! intermediate representation between the document model
//! (`editor-state`) and the DOM. Every visible piece of
//! content — a line, a marked span, a widget — is a tile.
//! Tiles know their `length` (in doc bytes) and their
//! `pos_at_start` (their offset in the document).
//!
//! ## Why we deviate from CM6's syntax (arena allocation)
//!
//! CM6 uses `tile.parent`, `tile.children` — direct references
//! cycling between parent and child. In Rust that needs
//! `Rc<RefCell<Tile>>` which adds allocations and runtime
//! borrow checking everywhere.
//!
//! We instead use a **flat arena**: every tile lives in
//! `Arena.tiles: Vec<Tile>` and references are `TileId(u32)`
//! indexes. Lookup is `&arena[id]`. Parent ↔ child traversal
//! becomes `arena[id].parent` (an `Option<TileId>`) and
//! `arena[id].children: Vec<TileId>`. No `Rc`, no `RefCell`,
//! no cycles in the borrow checker's view, faster.
//!
//! The **algorithms** are 1:1 with CM6. Only the syntax
//! differs. Anywhere this port reads less like the TS, expect
//! a comment naming the original method.
//!
//! ## Status
//!
//! Phase 1: this module — `Tile` struct, `TileKind` enum,
//! `TileFlag` bitflags, `Arena`, position math
//! (`pos_at_start`, `pos_at_end`, `pos_before`). Subsequent
//! phases add the concrete kinds (`TextTile`, `MarkTile`, etc.)
//! and the build pass.

pub mod arena;
pub mod build;
pub mod dom_pos;
pub mod flag;
pub mod line;
pub mod mark;
pub mod patch;
pub mod pos;
pub mod render_dx;
pub mod text;
pub mod visible;
pub mod widget;

pub use arena::{Arena, TileId};
pub use flag::TileFlag;

use crate::tile::flag::TileFlagSet;

/// Discriminator for the kind of tile. Every tile in the arena
/// carries one. Each variant's data lives in [`TileBody`]
/// alongside.
///
/// Mirrors CM6's class hierarchy: `Tile` ← `CompositeTile`
/// ← {`DocTile`, `LineTile`, `MarkTile`, `BlockWrapperTile`},
/// plus the leaves `TextTile`, `WidgetTile`, `WidgetBufferTile`.
///
/// We collapse the inheritance into a single enum because the
/// arena needs every entry to be the same size. The shared
/// fields (parent, length, flags, dom reference) live on
/// [`Tile`]; the kind-specific bits live in [`TileBody`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileKind {
    /// Root of the tree. One per editor view.
    Doc,
    /// One logical line. Composite — contains inline children.
    Line,
    /// Inline run of plain text. Leaf.
    Text,
    /// Wraps inline children with a mark decoration
    /// (`<span class="…">`). Composite.
    Mark,
    /// Inline widget (a non-document DOM range injected by a
    /// `WidgetDecoration`). Leaf.
    Widget,
    /// A zero-width inline widget placed *between* characters
    /// (CM6's "buffer widget" / point widget).
    WidgetBuffer,
    /// Block widget (full-line non-document content).
    /// Composite — its DOM children may include block widget
    /// internals.
    BlockWrapper,
}

/// Kind-specific data. Most variants carry no extra data; what
/// each `TileKind` adds is summarized in the variant docs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TileBody {
    /// Doc / Line / `BlockWrapper` bodies are all empty for now —
    /// their state lives in the shared `children` list. We
    /// extend as buildtile teaches us what each composite needs
    /// to carry.
    #[default]
    Empty,
    /// Inline text run. Holds the text itself.
    Text { text: String },
    /// Wrapper with a mark decoration applied to children
    /// (bold/italic/code/etc.).
    Mark { spec: mark::MarkSpec },
    /// Inline / block widget. v1 holds the rendered HTML
    /// (placeholder until we have a typed `Widget` trait).
    Widget { html: String },
    /// `LineTile` with extra CSS classes from `Decoration::line`.
    /// Space-joined onto the base `cm-line` class by the patcher.
    Line { extra_classes: Vec<String> },
}

/// One tile in the arena. Shared fields up top, kind in
/// `kind`, kind-specific extras in `body`, child indexes in
/// `children`.
///
/// Fields mirror CM6's `Tile` base class (`tile.ts:40-50`).
#[derive(Clone, Debug)]
pub struct Tile {
    /// Parent tile id, or `None` for the root [`TileKind::Doc`].
    pub parent: Option<TileId>,
    /// Ordered child tile ids. Empty for leaves.
    pub children: Vec<TileId>,
    /// Doc-length this tile represents. Composite tiles sum
    /// their children's lengths + `break_after` markers.
    pub length: usize,
    /// Discriminator.
    pub kind: TileKind,
    /// Kind-specific payload.
    pub body: TileBody,
    /// Bitflags — see [`TileFlag`].
    pub flags: TileFlagSet,
}

impl Tile {
    /// `true` when this tile has a `\n` immediately after it in
    /// the doc (CM6's `breakAfter` getter, derived from the
    /// `BreakAfter` flag).
    #[must_use]
    pub fn break_after(&self) -> bool {
        self.flags.contains(TileFlag::BreakAfter)
    }

    /// Type-narrowing helpers, mirroring `isText()`, `isLine()`,
    /// etc. from `tile.ts:55-65`. We return `bool` here; the
    /// arena gives us safe access to the body when we need it.
    #[must_use]
    pub fn is_text(&self) -> bool {
        matches!(self.kind, TileKind::Text)
    }
    #[must_use]
    pub fn is_line(&self) -> bool {
        matches!(self.kind, TileKind::Line)
    }
    #[must_use]
    pub fn is_widget(&self) -> bool {
        matches!(self.kind, TileKind::Widget | TileKind::WidgetBuffer)
    }
    #[must_use]
    pub fn is_composite(&self) -> bool {
        matches!(
            self.kind,
            TileKind::Doc | TileKind::Line | TileKind::Mark | TileKind::BlockWrapper
        )
    }
    #[must_use]
    pub fn is_block(&self) -> bool {
        matches!(
            self.kind,
            TileKind::Doc | TileKind::Line | TileKind::BlockWrapper
        )
    }
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        // Stand-in for CM6's `isHidden` getter — true when a
        // tile carries no visible content. Default false; a
        // specific Hidden-replace tile kind would set this. For
        // now we don't have one (we drop replaced ranges
        // entirely at build time).
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(kind: TileKind, length: usize) -> Tile {
        Tile {
            parent: None,
            children: Vec::new(),
            length,
            kind,
            body: TileBody::Empty,
            flags: TileFlagSet::empty(),
        }
    }

    #[test]
    fn break_after_reflects_flag() {
        let mut t = empty(TileKind::Line, 4);
        assert!(!t.break_after());
        t.flags.insert(TileFlag::BreakAfter);
        assert!(t.break_after());
    }

    #[test]
    fn kind_classifiers() {
        assert!(empty(TileKind::Text, 0).is_text());
        assert!(empty(TileKind::Mark, 0).is_composite());
        assert!(empty(TileKind::Line, 0).is_block());
        assert!(empty(TileKind::Widget, 0).is_widget());
        assert!(empty(TileKind::WidgetBuffer, 0).is_widget());
    }
}
