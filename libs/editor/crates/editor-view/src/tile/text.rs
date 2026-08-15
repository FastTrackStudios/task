//! `TextTile` constructor + accessors. The leaf tile for plain
//! text. CM6 reference: `tile.ts:383-426` (the `TextTile` class).
//!
//! In CM6 a `TextTile` owns its DOM Text node and a `sync()`
//! method writes `dom.nodeValue = this.text` when dirty. We don't
//! port `sync()` — Dioxus's reconciler handles the DOM text write
//! when we render the tile via rsx. Our job is to keep the model
//! correct; the view layer takes care of the DOM. (See
//! `editor::render_segment` for the current rendering shim; this
//! will move into `tile::render` in a later phase.)

use crate::tile::flag::TileFlagSet;
use crate::tile::{Tile, TileBody, TileKind};

/// Build a new [`Tile`] of [`TileKind::Text`] with the given
/// text. Length is the byte length. `parent` left `None`; the
/// caller wires parent/children when inserting into the arena.
///
/// Mirrors `TextTile.of` (`tile.ts:421-425`).
pub fn new_text_tile(text: impl Into<String>) -> Tile {
    let text = text.into();
    Tile {
        parent: None,
        children: Vec::new(),
        length: text.len(),
        kind: TileKind::Text,
        body: TileBody::Text { text },
        flags: TileFlagSet::empty(),
    }
}

/// Read the text out of a [`TileKind::Text`] tile. Panics if
/// called on a non-text tile.
#[must_use]
pub fn text_of(tile: &Tile) -> &str {
    match &tile.body {
        TileBody::Text { text } => text.as_str(),
        _ => panic!("text_of called on non-Text tile {:?}", tile.kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_text_sets_length_and_kind() {
        let t = new_text_tile("hello");
        assert_eq!(t.length, 5);
        assert!(t.is_text());
        assert_eq!(text_of(&t), "hello");
    }

    #[test]
    fn empty_text_has_zero_length() {
        let t = new_text_tile("");
        assert_eq!(t.length, 0);
        assert_eq!(text_of(&t), "");
    }

    #[test]
    #[should_panic(expected = "text_of called on non-Text tile")]
    fn text_of_panics_on_non_text() {
        let t = Tile {
            parent: None,
            children: Vec::new(),
            length: 0,
            kind: TileKind::Line,
            body: TileBody::Empty,
            flags: TileFlagSet::empty(),
        };
        let _ = text_of(&t);
    }
}
