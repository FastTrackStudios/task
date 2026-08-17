//! `LineTile` constructor. A composite tile representing one
//! logical line of the document. Renders as
//! `<div class="cm-line">`.
//!
//! CM6 reference: `tile.ts:286-355` (the `LineTile` class). We
//! port the constructor + the default `cm-line` class; the
//! `resolveInline` / `coordsIn` / `domIn` methods from CM6 are
//! satisfied by our generic [`crate::tile::dom_pos`] walker
//! since our v1 `LineTile` carries no per-line attrs beyond the
//! base class. Line decorations from extensions would extend
//! [`TileBody`] with `Line { attrs }` later.

use crate::tile::flag::TileFlagSet;
use crate::tile::{Tile, TileBody, TileKind};

/// Build a new [`Tile`] of [`TileKind::Line`]. Empty until
/// inline children are appended.
///
/// Mirrors `LineTile.start` (`tile.ts:293-297`).
#[must_use]
pub fn new_line_tile() -> Tile {
    Tile {
        parent: None,
        children: Vec::new(),
        length: 0,
        kind: TileKind::Line,
        body: TileBody::Line {
            extra_classes: Vec::new(),
        },
        flags: TileFlagSet::empty(),
    }
}

/// Append a class to the `LineTile`'s `extra_classes`. No-op if
/// the class is already present (idempotent — multiple Line
/// decorations of the same class collapse).
pub fn push_line_class(tile: &mut Tile, class: &str) {
    if let TileBody::Line { extra_classes } = &mut tile.body {
        if !extra_classes.iter().any(|c| c == class) {
            extra_classes.push(class.to_string());
        }
    }
}

/// Read the extra classes of a `LineTile`. Empty slice for tiles
/// without any Line decorations.
#[must_use]
pub fn line_extra_classes(tile: &Tile) -> &[String] {
    match &tile.body {
        TileBody::Line { extra_classes } => extra_classes,
        _ => &[],
    }
}
