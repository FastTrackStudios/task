//! The tile arena. All tiles for one editor view live in a
//! single `Vec<Tile>`; cross-tile references are `TileId`
//! indexes. This is the Rust-idiomatic alternative to CM6's
//! `tile.parent` / `tile.children` direct references.
//!
//! ## Why arena
//!
//! - No `Rc<RefCell<...>>`. Every borrow is checked at compile
//!   time once we have `&Arena` or `&mut Arena`.
//! - No fragmented allocation per tile — one growable Vec.
//! - Parent/child cycles in CM6's pointer graph become plain
//!   integers that the borrow checker is happy with.
//! - Identity is stable across moves: a `TileId` survives the
//!   arena growing or compacting (within the same generation).
//!
//! ## What's missing in v1
//!
//! - **Generation counters / slot recycling.** A removed
//!   tile's slot is just left as `None`. We'll add
//!   slot reuse + generation when memory becomes a
//!   concern; for now, editor docs are small and the
//!   arena resets per view.
//! - **Bulk destroy.** CM6's `destroy()` chains; we'll add a
//!   recursive remove once the tree mutators land.

use crate::tile::Tile;

/// Stable handle to a tile in the arena. `u32` is plenty —
/// a doc with more than 4 billion tiles is not on the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId(pub u32);

impl TileId {
    #[must_use]
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Slab of tiles. Each slot is either occupied or vacant
/// (after a future `remove`).
#[derive(Default, Debug, Clone)]
pub struct Arena {
    /// Tiles. `None` is a vacant slot; we don't reuse yet.
    tiles: Vec<Option<Tile>>,
}

impl Arena {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a tile, returning its id.
    pub fn insert(&mut self, tile: Tile) -> TileId {
        let id = TileId(self.tiles.len() as u32);
        self.tiles.push(Some(tile));
        id
    }

    /// Read access. Panics if the id is unknown — that's a
    /// bug in caller code, not a runtime error.
    #[must_use]
    pub fn get(&self, id: TileId) -> &Tile {
        self.tiles[id.as_usize()]
            .as_ref()
            .expect("TileId points to a vacant arena slot — destroy without rewiring")
    }

    /// Mutable access. Same panic contract as [`Self::get`].
    pub fn get_mut(&mut self, id: TileId) -> &mut Tile {
        self.tiles[id.as_usize()]
            .as_mut()
            .expect("TileId points to a vacant arena slot — destroy without rewiring")
    }

    /// Iterate every live tile id in insertion order. Skips
    /// vacant slots.
    pub fn iter_ids(&self) -> impl Iterator<Item = TileId> + '_ {
        self.tiles
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| TileId(i as u32)))
    }

    /// Total live tile count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tiles.iter().filter(|s| s.is_some()).count()
    }

    /// `true` when no live tiles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiles.iter().all(std::option::Option::is_none)
    }
}

impl std::ops::Index<TileId> for Arena {
    type Output = Tile;
    fn index(&self, id: TileId) -> &Tile {
        self.get(id)
    }
}

impl std::ops::IndexMut<TileId> for Arena {
    fn index_mut(&mut self, id: TileId) -> &mut Tile {
        self.get_mut(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::flag::TileFlagSet;
    use crate::tile::{TileBody, TileKind};

    fn tile(kind: TileKind, length: usize) -> Tile {
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
    fn insert_and_read_back() {
        let mut a = Arena::new();
        let id = a.insert(tile(TileKind::Line, 10));
        assert_eq!(a[id].length, 10);
        assert!(a[id].is_line());
    }

    #[test]
    fn iter_ids_yields_insert_order() {
        let mut a = Arena::new();
        let a_id = a.insert(tile(TileKind::Text, 1));
        let b_id = a.insert(tile(TileKind::Text, 2));
        let ids: Vec<_> = a.iter_ids().collect();
        assert_eq!(ids, vec![a_id, b_id]);
    }
}
