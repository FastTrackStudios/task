//! `WidgetTile` + `WidgetBufferTile` constructors.
//!
//! A `WidgetTile` is an inline DOM range that *isn't* part of
//! the document text — a checkbox in front of a task, an image
//! preview next to a markdown link, etc. CM6 reference:
//! `tile.ts:428-483` for `WidgetTile`, `:488-505` for
//! `WidgetBufferTile`.
//!
//! In CM6, `WidgetTile.widget` is a typed [`WidgetType`]
//! instance with `toDOM`, `eq`, `coordsAt`, `destroy`, etc.
//! Our v1 stores the rendered HTML string in `TileBody::Widget`
//! (already there) and accepts widgets via a trait + a
//! materialized HTML string at build time. The richer trait
//! lands when we wire buildtile.

use crate::tile::flag::{TileFlag, TileFlagSet};
use crate::tile::{Tile, TileBody, TileKind};

/// Build a [`TileKind::Widget`] tile. `length` is the doc
/// length the widget *replaces* — 0 for an inline point widget
/// (sits between characters), positive for a replace widget
/// that covers a doc range. `flags` carries the
/// `Before`/`After`/`IncStart`/`IncEnd`/`Block` widget bits
/// (see [`TileFlag`]).
///
/// Mirrors `WidgetTile.of` (`tile.ts:476-482`).
pub fn new_widget_tile(html: impl Into<String>, length: usize, flags: TileFlagSet) -> Tile {
    Tile {
        parent: None,
        children: Vec::new(),
        length,
        kind: TileKind::Widget,
        body: TileBody::Widget { html: html.into() },
        flags,
    }
}

/// Build a [`TileKind::WidgetBuffer`] tile. These are tiny
/// invisible `<img>` placeholders CM6 inserts adjacent to
/// non-editable widgets to work around browser cursor-bugs
/// next to uneditable inline content.
///
/// Mirrors `WidgetBufferTile` (`tile.ts:488-505`).
#[must_use]
pub fn new_widget_buffer_tile(flags: TileFlagSet) -> Tile {
    Tile {
        parent: None,
        children: Vec::new(),
        length: 0,
        kind: TileKind::WidgetBuffer,
        body: TileBody::Widget {
            html: String::new(),
        },
        flags,
    }
}

/// `true` when this is a *point* widget (sits between
/// characters, doesn't replace doc bytes). Mirrors CM6's
/// `flags & PointWidget` test (`tile.ts:440`).
#[must_use]
pub fn is_point_widget(tile: &Tile) -> bool {
    tile.flags.intersects(TileFlag::POINT_WIDGET)
}

/// `true` when the widget extends/covers the position on the
/// given side — controls boundary collapse for selection
/// movement. Mirrors `WidgetTile.covers` (`tile.ts:439-442`).
#[must_use]
pub fn widget_covers(tile: &Tile, side_negative: bool) -> bool {
    if is_point_widget(tile) {
        return false;
    }
    let flag = if side_negative {
        TileFlag::IncStart
    } else {
        TileFlag::IncEnd
    };
    tile.flags.contains(flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_construction() {
        let mut flags = TileFlagSet::empty();
        flags.insert(TileFlag::IncStart);
        flags.insert(TileFlag::IncEnd);
        let t = new_widget_tile("<i class=icon/>", 4, flags);
        assert!(t.is_widget());
        assert_eq!(t.length, 4);
    }

    #[test]
    fn point_widget_zero_length() {
        let mut flags = TileFlagSet::empty();
        flags.insert(TileFlag::Before);
        let t = new_widget_tile("<span/>", 0, flags);
        assert!(is_point_widget(&t));
    }

    #[test]
    fn covers_respects_inclusivity_flag() {
        // Replace widget inclusive at start only.
        let mut flags = TileFlagSet::empty();
        flags.insert(TileFlag::IncStart);
        let t = new_widget_tile("<x/>", 2, flags);
        assert!(widget_covers(&t, true)); // side = -1
        assert!(!widget_covers(&t, false)); // side = +1
    }

    #[test]
    fn point_widget_never_covers() {
        let mut flags = TileFlagSet::empty();
        flags.insert(TileFlag::Before);
        flags.insert(TileFlag::IncStart);
        let t = new_widget_tile("<x/>", 0, flags);
        // Even with IncStart set, point widgets don't cover —
        // they sit between characters.
        assert!(!widget_covers(&t, true));
    }

    #[test]
    fn widget_buffer_is_hidden_zero_length() {
        let t = new_widget_buffer_tile(TileFlagSet::empty());
        assert_eq!(t.length, 0);
        assert!(t.is_widget());
    }
}
