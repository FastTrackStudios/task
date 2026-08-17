//! `MarkTile` constructor + accessors. A composite tile that
//! wraps inline children with a mark decoration — renders as
//! `<span class="…">`.
//!
//! CM6 reference: `tile.ts:369-381` (the `MarkTile` class) and
//! its associated `MarkDecoration` spec (`decoration.ts`).
//!
//! In CM6, `MarkTile.mark` carries a `MarkDecoration` instance
//! which holds the tag name (`span` by default), CSS class,
//! inclusivity flags, and an `attrs` getter. We collapse that
//! into a Rust `MarkSpec` value embedded in [`TileBody`]…
//! actually, our v1 only needs the class string and inclusivity
//! flags. We extend [`TileBody`] gradually as we need more.

use crate::tile::flag::TileFlagSet;
use crate::tile::{Tile, TileBody, TileKind};

/// What a [`TileKind::Mark`] tile needs to render itself: the
/// CSS class, the HTML tag, and inclusivity hints used during
/// build-time merging. Stored inside `TileBody::Mark`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MarkSpec {
    /// HTML tag for the wrapping element. Defaults to "span".
    pub tag: String,
    /// CSS class on the wrapper. Multiple classes are
    /// space-joined.
    pub class: String,
    /// Extra HTML attributes copied verbatim onto the rendered
    /// element. Used for things like `data-href` on link spans
    /// so the JS click handler knows where to navigate.
    pub attrs: Vec<(String, String)>,
    /// Inclusive at start — typing at the boundary becomes
    /// part of the mark.
    pub inclusive_start: bool,
    /// Inclusive at end.
    pub inclusive_end: bool,
}

impl MarkSpec {
    /// Default `<span>` mark with a single class.
    pub fn span_class(class: impl Into<String>) -> Self {
        Self {
            tag: "span".into(),
            class: class.into(),
            attrs: Vec::new(),
            inclusive_start: false,
            inclusive_end: false,
        }
    }
    #[must_use]
    pub fn with_attrs(mut self, attrs: Vec<(String, String)>) -> Self {
        self.attrs = attrs;
        self
    }
}

/// Build a new [`Tile`] of [`TileKind::Mark`]. Length is the
/// sum of child lengths; we leave it 0 here and let the
/// builder fix it up after children are attached.
///
/// Mirrors `MarkTile.of` (`tile.ts:376-380`). The DOM element
/// is produced by Dioxus at render-time; we just record the
/// spec so the renderer knows how to wrap children.
#[must_use]
pub fn new_mark_tile(spec: MarkSpec) -> Tile {
    Tile {
        parent: None,
        children: Vec::new(),
        length: 0,
        kind: TileKind::Mark,
        body: TileBody::Mark { spec },
        flags: TileFlagSet::empty(),
    }
}

/// Read the spec out of a [`TileKind::Mark`] tile. Panics if
/// called on a non-mark tile.
#[must_use]
pub fn mark_spec_of(tile: &Tile) -> &MarkSpec {
    match &tile.body {
        TileBody::Mark { spec } => spec,
        _ => panic!("mark_spec_of called on non-Mark tile {:?}", tile.kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_mark_creates_composite_tile() {
        let t = new_mark_tile(MarkSpec::span_class("md-bold"));
        assert!(t.is_composite());
        assert_eq!(t.length, 0);
        assert!(t.children.is_empty());
    }

    #[test]
    fn mark_spec_defaults() {
        let s = MarkSpec::span_class("italic");
        assert_eq!(s.tag, "span");
        assert!(!s.inclusive_start && !s.inclusive_end);
    }

    #[test]
    fn mark_spec_round_trips_through_tile() {
        let t = new_mark_tile(MarkSpec::span_class("md-bold"));
        assert_eq!(mark_spec_of(&t).class, "md-bold");
    }
}
