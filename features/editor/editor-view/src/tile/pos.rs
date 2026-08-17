//! Position math on the tile tree.
//!
//! Three concepts:
//!
//! - **`pos_at_start(tile)`** — the doc byte offset where this
//!   tile begins. Computed by walking up to the root summing
//!   sibling lengths. CM6: `Tile.posAtStart` getter
//!   (`tile.ts:89`).
//! - **`pos_at_end(tile)`** — `pos_at_start + length`.
//!   CM6: `Tile.posAtEnd` (`tile.ts:93`).
//! - **`pos_before(parent, child)`** — the doc offset where
//!   `child` starts within `parent`'s coordinate space.
//!   CM6: `Tile.posBefore` (`tile.ts:97`).
//!
//! All three are pure functions on (`Arena`, `TileId`). They
//! don't mutate state — they reconstruct positions from the
//! current tree shape on each call. CM6 likewise recomputes
//! `posAtStart` each call; caching is a future optimization
//! once we measure it.

use crate::tile::arena::{Arena, TileId};

/// Doc offset where `tile` starts. Mirrors CM6's
/// `Tile.posAtStart` getter (`tile.ts:89-91`).
///
/// Implementation walks rootward: each step adds the offset
/// of `current` within its parent (= sum of previous siblings'
/// `length + break_after`).
#[must_use]
pub fn pos_at_start(arena: &Arena, tile: TileId) -> usize {
    let mut current = tile;
    let mut acc = 0usize;
    while let Some(parent) = arena[current].parent {
        acc += pos_before(arena, parent, current);
        current = parent;
    }
    acc
}

/// `pos_at_start(tile) + tile.length`. Mirrors CM6's
/// `Tile.posAtEnd` (`tile.ts:93-95`).
#[must_use]
pub fn pos_at_end(arena: &Arena, tile: TileId) -> usize {
    pos_at_start(arena, tile) + arena[tile].length
}

/// Doc offset of `child` within `parent`. Mirrors CM6's
/// `Tile.posBefore` (`tile.ts:97-104`). Panics if `child`
/// isn't actually a child of `parent` — caller bug.
#[must_use]
pub fn pos_before(arena: &Arena, parent: TileId, child: TileId) -> usize {
    let mut pos = 0;
    for &sib in &arena[parent].children {
        if sib == child {
            return pos;
        }
        pos += arena[sib].length + usize::from(arena[sib].break_after());
    }
    panic!("pos_before: {child:?} is not a child of {parent:?}");
}

/// Doc offset of the *character after* `child` within
/// `parent`'s coordinate space. Mirrors CM6's
/// `Tile.posAfter` (`tile.ts:106-108`).
#[must_use]
pub fn pos_after(arena: &Arena, parent: TileId, child: TileId) -> usize {
    pos_before(arena, parent, child) + arena[child].length
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::flag::{TileFlag, TileFlagSet};
    use crate::tile::{Tile, TileBody, TileKind};

    /// Build a small tree:
    ///   Doc (root, length=11)
    ///   ├── Line "hello" (length=5, breakAfter)
    ///   └── Line "world!" (length=6)
    ///
    /// Returns `(arena, doc_id, line1_id, line2_id)`.
    fn build_two_line_tree() -> (Arena, TileId, TileId, TileId) {
        let mut a = Arena::new();
        let doc = a.insert(Tile {
            parent: None,
            children: vec![],
            length: 0, // will compute below
            kind: TileKind::Doc,
            body: TileBody::Empty,
            flags: TileFlagSet::empty(),
        });
        let l1 = a.insert(Tile {
            parent: Some(doc),
            children: vec![],
            length: 5,
            kind: TileKind::Line,
            body: TileBody::Empty,
            flags: {
                let mut f = TileFlagSet::empty();
                f.insert(TileFlag::BreakAfter);
                f
            },
        });
        let l2 = a.insert(Tile {
            parent: Some(doc),
            children: vec![],
            length: 6,
            kind: TileKind::Line,
            body: TileBody::Empty,
            flags: TileFlagSet::empty(),
        });
        // length = sum(child.length + child.break_after)
        a[doc].children = vec![l1, l2];
        a[doc].length = 5 + 1 + 6;
        (a, doc, l1, l2)
    }

    #[test]
    fn doc_starts_at_zero() {
        let (arena, doc, _, _) = build_two_line_tree();
        assert_eq!(pos_at_start(&arena, doc), 0);
    }

    #[test]
    fn line1_starts_at_zero_within_doc() {
        let (arena, _doc, l1, _) = build_two_line_tree();
        assert_eq!(pos_at_start(&arena, l1), 0);
        assert_eq!(pos_at_end(&arena, l1), 5);
    }

    #[test]
    fn line2_starts_past_line1_and_its_break() {
        let (arena, _doc, _l1, l2) = build_two_line_tree();
        // l1 occupies 0..5, the break takes byte 5, so l2
        // starts at 6.
        assert_eq!(pos_at_start(&arena, l2), 6);
        assert_eq!(pos_at_end(&arena, l2), 12);
    }

    #[test]
    fn pos_before_walks_siblings() {
        let (arena, doc, l1, l2) = build_two_line_tree();
        assert_eq!(pos_before(&arena, doc, l1), 0);
        assert_eq!(pos_before(&arena, doc, l2), 6);
    }

    #[test]
    fn pos_after_uses_child_length() {
        let (arena, doc, l1, _) = build_two_line_tree();
        // l1's content ends at 5 (the break itself isn't part
        // of l1's length).
        assert_eq!(pos_after(&arena, doc, l1), 5);
    }

    #[test]
    fn nested_pos_at_start_reaches_grandparent() {
        // Build:
        //   Doc
        //   └── Line (length=5)
        //         └── Text "hello" (length=5)
        let mut a = Arena::new();
        let doc = a.insert(Tile {
            parent: None,
            children: vec![],
            length: 5,
            kind: TileKind::Doc,
            body: TileBody::Empty,
            flags: TileFlagSet::empty(),
        });
        let line = a.insert(Tile {
            parent: Some(doc),
            children: vec![],
            length: 5,
            kind: TileKind::Line,
            body: TileBody::Empty,
            flags: TileFlagSet::empty(),
        });
        let text = a.insert(Tile {
            parent: Some(line),
            children: vec![],
            length: 5,
            kind: TileKind::Text,
            body: TileBody::Text {
                text: "hello".into(),
            },
            flags: TileFlagSet::empty(),
        });
        a[doc].children = vec![line];
        a[line].children = vec![text];

        assert_eq!(pos_at_start(arena_view(&a), text), 0);
        assert_eq!(pos_at_end(arena_view(&a), text), 5);
    }

    /// Tiny helper for the nested test — clones the arena
    /// borrow so the test reads cleaner.
    fn arena_view(a: &Arena) -> &Arena {
        a
    }
}
