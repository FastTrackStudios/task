//! Selections — one or more ranges in the document, plus a
//! primary index. Each range carries an `anchor` and `head`; the
//! caret is at `head` and the selection extends from `anchor`.
//!
//! Mirrors `@codemirror/state`'s `EditorSelection` /
//! `SelectionRange`. Multi-range support is here from day one
//! (so we can grow into multi-cursor later) but the v1 view only
//! renders the primary range.
//!
//! Reference: `~/Development/research/codemirror/state/src/selection.ts`.

use crate::change::{Assoc, Changes};

/// One selection range. When `anchor == head`, it's a caret.
/// When `anchor != head`, it covers `min(anchor, head)..max(anchor, head)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    pub anchor: usize,
    pub head: usize,
}

impl Range {
    /// A caret at `pos`.
    #[must_use]
    pub fn caret(pos: usize) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }

    /// A range with explicit endpoints. `anchor` is where the
    /// selection started, `head` is where the caret is.
    #[must_use]
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// `true` when `anchor == head`.
    #[must_use]
    pub fn is_caret(self) -> bool {
        self.anchor == self.head
    }

    /// Inclusive-start, exclusive-end byte range covering the
    /// selection. `from..to` with `from <= to`.
    #[must_use]
    pub fn byte_range(self) -> std::ops::Range<usize> {
        let lo = self.anchor.min(self.head);
        let hi = self.anchor.max(self.head);
        lo..hi
    }

    /// Forward-direction `from`. Equivalent to `byte_range().start`.
    #[must_use]
    pub fn from(self) -> usize {
        self.anchor.min(self.head)
    }

    /// Forward-direction `to`. Equivalent to `byte_range().end`.
    #[must_use]
    pub fn to(self) -> usize {
        self.anchor.max(self.head)
    }

    /// Map both endpoints through a change set.
    ///
    /// - For a **caret** (`anchor == head`), both endpoints
    ///   follow the typing cursor — `After` bias for both. A
    ///   user typing at position 5 then inserting "x" should
    ///   end with caret at 6, not a 5..6 selection.
    /// - For a **non-caret** selection, the anchor is a fixed
    ///   pivot (`Before`) and the head extends with following
    ///   text (`After`).
    #[must_use]
    pub fn map(self, changes: &Changes) -> Self {
        let (anchor_bias, head_bias) = if self.anchor == self.head {
            (Assoc::After, Assoc::After)
        } else {
            (Assoc::Before, Assoc::After)
        };
        Self {
            anchor: changes.map_position(self.anchor, anchor_bias),
            head: changes.map_position(self.head, head_bias),
        }
    }
}

/// A non-empty list of [`Range`]s plus an index pointing at the
/// "primary" one. For single-cursor use, the list has one entry
/// and `primary == 0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    ranges: Vec<Range>,
    primary: usize,
}

impl Selection {
    /// A single-range selection — by far the common case.
    #[must_use]
    pub fn single(range: Range) -> Self {
        Self {
            ranges: vec![range],
            primary: 0,
        }
    }

    /// A single caret at `pos`.
    #[must_use]
    pub fn caret(pos: usize) -> Self {
        Self::single(Range::caret(pos))
    }

    /// Multi-range selection. Panics if `ranges` is empty or
    /// `primary` is out of bounds.
    #[must_use]
    pub fn many(ranges: Vec<Range>, primary: usize) -> Self {
        assert!(!ranges.is_empty(), "Selection must have at least one range");
        assert!(primary < ranges.len(), "primary index out of bounds");
        Self { ranges, primary }
    }

    /// The "active" range — where the caret is for cursor-aware
    /// behaviors (active-line highlight, format-text!, etc.).
    #[must_use]
    pub fn primary(&self) -> Range {
        self.ranges[self.primary]
    }

    /// All ranges in order.
    #[must_use]
    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    /// Index of the primary range in `ranges()`.
    #[must_use]
    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// Map every range through a change set. Used by
    /// `Transaction::apply` to keep selections valid across edits.
    #[must_use]
    pub fn map(&self, changes: &Changes) -> Self {
        Self {
            ranges: self.ranges.iter().map(|r| r.map(changes)).collect(),
            primary: self.primary,
        }
    }
}

impl Default for Selection {
    fn default() -> Self {
        Self::caret(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::{Change, Changes};

    #[test]
    fn caret_range_is_zero_width() {
        let r = Range::caret(5);
        assert!(r.is_caret());
        assert_eq!(r.byte_range(), 5..5);
    }

    #[test]
    fn range_from_to_normalizes() {
        let r = Range::new(10, 3); // backward selection
        assert_eq!(r.from(), 3);
        assert_eq!(r.to(), 10);
    }

    #[test]
    fn map_caret_through_earlier_insert() {
        let cs = Changes::insert(0, "X");
        let r = Range::caret(5).map(&cs);
        assert_eq!(r.head, 6);
        assert_eq!(r.anchor, 6);
    }

    #[test]
    fn map_selection_through_replacement() {
        // "abc[def]gh" — selection 3..6, replace 0..3 with "ZZ" → "ZZ[def]gh".
        let cs = Changes::single(Change::replace(0..3, "ZZ"));
        let sel = Selection::single(Range::new(3, 6)).map(&cs);
        assert_eq!(sel.primary(), Range::new(2, 5));
    }
}
