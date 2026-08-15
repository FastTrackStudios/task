//! DOM ↔ doc position bridging, the way CM6 does it in
//! `docview.ts:282-323` (`posFromDOM` + `domAtPos`).
//!
//! `pos_from_tile(arena, tile_id, offset)` — given a tile and
//! an offset *within that tile's content*, returns the doc
//! position. This is what `posFromDOM` boils down to once
//! you've walked the DOM to find the owning tile via
//! `data-tile-id` attributes (the data-attr equivalent of
//! CM6's `dom.cmTile` expando — see `tile.ts:11-12`).
//!
//! `tile_at_pos(arena, doc_id, pos, side)` — given a doc
//! position and a side bias, finds the tile that owns it.
//! This is CM6's `domAtPos` direction. The `side` parameter
//! matches CM6's `-1 | 1` argument: at a boundary between two
//! tiles, prefer the one on this side.

use crate::tile::arena::{Arena, TileId};
use crate::tile::pos::pos_at_start;

/// Side bias for boundary resolution. Mirrors CM6's
/// `side: -1 | 1` (`docview.ts:320`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Boundary belongs to the tile on the left.
    Before,
    /// Boundary belongs to the tile on the right.
    After,
}

/// Doc position given a tile + offset within its content.
/// `offset` is in the tile's local coordinate space; for a
/// leaf `TextTile` that's the character offset in its text.
///
/// Mirrors `Tile.posAtStart + offset` from CM6's
/// `posFromDOM` (`docview.ts:282`).
#[must_use]
pub fn pos_from_tile(arena: &Arena, tile: TileId, offset: usize) -> usize {
    pos_at_start(arena, tile) + offset
}

/// Resolve a doc position to the owning tile + local offset.
/// Walks down from `root` looking for the tile whose
/// `[pos_at_start, pos_at_end]` range contains `pos`. The
/// `side` parameter decides boundary cases.
///
/// Returns `(tile_id, offset_within_tile)`. For composite
/// tiles the returned offset is in the composite's coordinate
/// space; callers that need a leaf descend further via
/// [`leaf_at_pos`].
///
/// Mirrors CM6's `Tile.children` walk in `domAtPos`
/// (`docview.ts:320-323`).
#[must_use]
pub fn tile_at_pos(arena: &Arena, root: TileId, pos: usize, side: Side) -> (TileId, usize) {
    descend(arena, root, pos, side)
}

/// Find the leaf tile owning `pos` and return `(leaf_id,
/// offset_within_leaf)`. Same as `tile_at_pos` but recurses
/// until it hits a leaf.
#[must_use]
pub fn leaf_at_pos(arena: &Arena, root: TileId, pos: usize, side: Side) -> (TileId, usize) {
    let mut cur = root;
    let mut local = pos.saturating_sub(pos_at_start(arena, root));
    loop {
        let children = &arena.get(cur).children;
        if children.is_empty() {
            return (cur, local.min(arena.get(cur).length));
        }
        // Walk children to find the one containing `local`.
        let mut acc = 0usize;
        let mut chosen: Option<TileId> = None;
        for (idx, &child) in children.iter().enumerate() {
            let child_len = arena.get(child).length + usize::from(arena.get(child).break_after());
            let next_acc = acc + child_len;
            let inside = if side == Side::Before {
                local > acc && local <= next_acc
            } else {
                local >= acc && local < next_acc
            };
            if inside {
                chosen = Some(child);
                local -= acc;
                break;
            }
            // Last child + position at the tail: use it.
            if idx + 1 == children.len() && local <= next_acc {
                chosen = Some(child);
                local -= acc;
                break;
            }
            acc = next_acc;
        }
        match chosen {
            Some(c) => cur = c,
            None => return (cur, local.min(arena.get(cur).length)),
        }
    }
}

fn descend(arena: &Arena, root: TileId, pos: usize, side: Side) -> (TileId, usize) {
    leaf_at_pos(arena, root, pos, side)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::build::build_tiles;
    use editor_state::Decoration;

    /// Helper: the (sole) `LineTile` under the doc.
    fn first_line(arena: &Arena, doc: TileId) -> TileId {
        arena.get(doc).children[0]
    }

    #[test]
    fn pos_from_tile_plain_text() {
        // "hello" → Doc → Line → Text(0..5). Tile + offset 3
        // → pos 3 (LineTile contributes 0 since it's at start).
        let (arena, doc) = build_tiles("hello", &[]);
        let text = arena.get(first_line(&arena, doc)).children[0];
        assert_eq!(pos_from_tile(&arena, text, 3), 3);
    }

    #[test]
    fn leaf_at_pos_in_plain_text() {
        let (arena, doc) = build_tiles("hello", &[]);
        let text = arena.get(first_line(&arena, doc)).children[0];
        let (got, off) = leaf_at_pos(&arena, doc, 3, Side::After);
        assert_eq!(got, text);
        assert_eq!(off, 3);
    }

    #[test]
    fn leaf_at_pos_after_mark() {
        let decs = vec![Decoration::mark(2..4, "bold")];
        let (arena, doc) = build_tiles("hello", &decs);
        let line = first_line(&arena, doc);
        let mark = arena.get(line).children[1];
        let inner_text = arena.get(mark).children[0];
        let (got, off) = leaf_at_pos(&arena, doc, 3, Side::After);
        assert_eq!(got, inner_text);
        assert_eq!(off, 1);
    }

    #[test]
    fn leaf_at_pos_at_boundary_side_after_picks_right() {
        let decs = vec![Decoration::mark(2..4, "bold")];
        let (arena, doc) = build_tiles("hello", &decs);
        let line = first_line(&arena, doc);
        let mark = arena.get(line).children[1];
        let inner_text = arena.get(mark).children[0];
        let (got, off) = leaf_at_pos(&arena, doc, 2, Side::After);
        assert_eq!(got, inner_text);
        assert_eq!(off, 0);
    }

    #[test]
    fn leaf_at_pos_at_boundary_side_before_picks_left() {
        let decs = vec![Decoration::mark(2..4, "bold")];
        let (arena, doc) = build_tiles("hello", &decs);
        let line = first_line(&arena, doc);
        let first_text = arena.get(line).children[0];
        let (got, off) = leaf_at_pos(&arena, doc, 2, Side::Before);
        assert_eq!(got, first_text);
        assert_eq!(off, 2);
    }

    #[test]
    fn leaf_at_pos_with_replace_lands_on_hidden() {
        let decs = vec![Decoration::replace(0..2)];
        let (arena, doc) = build_tiles("**bold**", &decs);
        let line = first_line(&arena, doc);
        let hidden = arena.get(line).children[0];
        let (got, _off) = leaf_at_pos(&arena, doc, 1, Side::After);
        assert_eq!(got, hidden);
    }

    #[test]
    fn pos_at_end_of_doc_is_doc_length() {
        use crate::tile::pos::pos_at_end;
        let (arena, doc) = build_tiles("hello", &[]);
        assert_eq!(pos_at_end(&arena, doc), 5);
    }
}
