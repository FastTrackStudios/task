//! Tile bitflags. 1:1 port of CM6's `TileFlag` enum
//! (`view/src/tile.ts:18-36`). We use a hand-rolled bitflag
//! type to keep the editor-view crate dep-light; if the API
//! grows large we can swap in the `bitflags` crate.

/// One flag in [`TileFlagSet`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum TileFlag {
    /// There's a `\n` immediately after this tile in the doc.
    /// One position of length is owned by the break, not the
    /// tile itself — that's how CM6 keeps line-end carets
    /// distinguishable.
    BreakAfter = 1,
    /// DOM has been synced with the tile's logical content.
    /// Cleared when something marks the tile dirty.
    Synced = 2,
    /// Attributes on the tile's DOM element are stale.
    AttrsDirty = 4,
    /// This tile is the active composition input. Prevents
    /// text-merging while the IME owns its text node.
    Composition = 8,

    // Widget flags. The first two are mutually exclusive — a
    // point widget sits either *before* the cursor position or
    // *after* it for boundary-collapse purposes.
    /// Single-point widget before cursor.
    Before = 16,
    /// Single-point widget after cursor.
    After = 32,
    /// Replace-widget inclusivity at start.
    IncStart = 64,
    /// Replace-widget inclusivity at end.
    IncEnd = 128,
    /// Distinguishes block widgets from inline widgets.
    Block = 256,
}

impl TileFlag {
    /// Mask for "any of the point-widget flags."
    pub const POINT_WIDGET: u16 = TileFlag::Before as u16 | TileFlag::After as u16;
    /// Mask for "any widget-related flag."
    pub const WIDGET: u16 = TileFlag::Before as u16
        | TileFlag::After as u16
        | TileFlag::IncStart as u16
        | TileFlag::IncEnd as u16
        | TileFlag::Block as u16;
}

/// A set of [`TileFlag`]s. Constructed via `empty()` + `insert`,
/// or directly from a `u16` of OR'd flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TileFlagSet(pub u16);

impl TileFlagSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub fn contains(self, flag: TileFlag) -> bool {
        (self.0 & flag as u16) != 0
    }

    pub fn insert(&mut self, flag: TileFlag) {
        self.0 |= flag as u16;
    }

    pub fn remove(&mut self, flag: TileFlag) {
        self.0 &= !(flag as u16);
    }

    #[must_use]
    pub fn intersects(self, mask: u16) -> bool {
        (self.0 & mask) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_contains() {
        let mut s = TileFlagSet::empty();
        s.insert(TileFlag::BreakAfter);
        assert!(s.contains(TileFlag::BreakAfter));
        assert!(!s.contains(TileFlag::Synced));
    }

    #[test]
    fn remove_clears_only_the_flag() {
        let mut s = TileFlagSet::empty();
        s.insert(TileFlag::BreakAfter);
        s.insert(TileFlag::Synced);
        s.remove(TileFlag::BreakAfter);
        assert!(!s.contains(TileFlag::BreakAfter));
        assert!(s.contains(TileFlag::Synced));
    }

    #[test]
    fn widget_mask_covers_only_widget_flags() {
        let mut s = TileFlagSet::empty();
        s.insert(TileFlag::Before);
        assert!(s.intersects(TileFlag::WIDGET));
        assert!(s.intersects(TileFlag::POINT_WIDGET));
        let mut s2 = TileFlagSet::empty();
        s2.insert(TileFlag::Synced);
        assert!(!s2.intersects(TileFlag::WIDGET));
    }
}
