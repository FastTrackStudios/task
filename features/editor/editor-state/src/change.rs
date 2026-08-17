//! Changes — the atomic edit operations applied as part of a
//! [`crate::Transaction`]. A `Changes` is an ordered list of
//! non-overlapping `Change`s sorted by position.
//!
//! Mirrors `@codemirror/state`'s `ChangeSet`. The key operation
//! beyond apply-to-doc is **position mapping**: given a byte
//! offset that was valid before the change, what offset does it
//! correspond to *after* the change? Selections, decorations,
//! and CRDT cursors all rely on this.
//!
//! Reference: `~/Development/research/codemirror/state/src/change.ts`.

use crate::doc::Doc;

/// One atomic edit. Either an insertion at a position, or a
/// deletion of a byte range, or a replacement (both).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Byte offset where the change starts.
    pub from: usize,
    /// Byte offset where the change ends. For a pure insert,
    /// `to == from`. For a pure delete, `inserted.is_empty()`.
    pub to: usize,
    /// Text inserted at `from`. Empty for pure deletes.
    pub inserted: String,
}

impl Change {
    /// Pure insertion at `at`.
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self {
            from: at,
            to: at,
            inserted: text.into(),
        }
    }

    /// Pure deletion of a byte range.
    #[must_use]
    pub fn delete(range: std::ops::Range<usize>) -> Self {
        Self {
            from: range.start,
            to: range.end,
            inserted: String::new(),
        }
    }

    /// Replace a byte range with new text.
    pub fn replace(range: std::ops::Range<usize>, text: impl Into<String>) -> Self {
        Self {
            from: range.start,
            to: range.end,
            inserted: text.into(),
        }
    }

    /// Net length change: positive = doc grows, negative = doc
    /// shrinks.
    #[must_use]
    pub fn delta(&self) -> isize {
        self.inserted.len() as isize - (self.to as isize - self.from as isize)
    }
}

/// An ordered, non-overlapping list of [`Change`]s, sorted by
/// `from`. Build via [`Changes::single`] or
/// [`Changes::from_sorted`]. Apply with [`Changes::apply`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    inner: Vec<Change>,
}

impl Changes {
    /// Empty change set — applying it is a no-op.
    #[must_use]
    pub fn empty() -> Self {
        Self { inner: Vec::new() }
    }

    /// Convenience: a single change.
    #[must_use]
    pub fn single(c: Change) -> Self {
        Self { inner: vec![c] }
    }

    /// Build from a pre-sorted, non-overlapping list of changes.
    /// Panics in debug if the invariant is violated — caller's
    /// responsibility to maintain.
    #[must_use]
    pub fn from_sorted(changes: Vec<Change>) -> Self {
        #[cfg(debug_assertions)]
        {
            for w in changes.windows(2) {
                assert!(
                    w[0].to <= w[1].from,
                    "Changes must be sorted and non-overlapping (got {:?} then {:?})",
                    w[0],
                    w[1]
                );
            }
        }
        Self { inner: changes }
    }

    /// Iterate the underlying changes in order.
    pub fn iter(&self) -> impl Iterator<Item = &Change> {
        self.inner.iter()
    }

    /// `true` when there's nothing to apply.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Convenience constructor: pure insertion.
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::single(Change::insert(at, text))
    }

    /// Convenience constructor: pure deletion.
    #[must_use]
    pub fn delete(range: std::ops::Range<usize>) -> Self {
        Self::single(Change::delete(range))
    }

    /// Convenience constructor: replacement.
    pub fn replace(range: std::ops::Range<usize>, text: impl Into<String>) -> Self {
        Self::single(Change::replace(range, text))
    }

    /// Apply to a document and return the resulting doc.
    #[must_use]
    pub fn apply(&self, doc: &Doc) -> Doc {
        // Walk changes in reverse so earlier offsets aren't
        // invalidated by later edits.
        let mut out = doc.clone();
        for change in self.inner.iter().rev() {
            if change.from != change.to {
                out = out.delete(change.from..change.to);
            }
            if !change.inserted.is_empty() {
                out = out.insert(change.from, &change.inserted);
            }
        }
        out
    }

    /// Invert this change set against the document it was applied
    /// *to* (the pre-change doc). The result is a change set valid
    /// on the *post*-change document; applying it restores `doc`.
    ///
    /// Mirrors `ChangeSet.invert` in `@codemirror/state` — the
    /// history stores inverted change sets so undo is just "apply".
    ///
    /// Each change `{from, to, inserted}` becomes
    /// `{from', from' + inserted.len(), doc[from..to]}` where
    /// `from'` is `from` shifted by the cumulative delta of the
    /// changes before it. Sortedness and non-overlap are preserved.
    #[must_use]
    pub fn invert(&self, doc: &Doc) -> Changes {
        let mut shift: isize = 0;
        let inverted = self
            .inner
            .iter()
            .map(|c| {
                let from = (c.from as isize + shift) as usize;
                let inv = Change {
                    from,
                    to: from + c.inserted.len(),
                    inserted: doc.slice(c.from..c.to),
                };
                shift += c.delta();
                inv
            })
            .collect();
        Changes::from_sorted(inverted)
    }

    /// Map a byte offset through this change set. Used by
    /// selections and decorations to stay anchored across edits.
    ///
    /// `assoc` controls behavior at the exact start of an
    /// insertion:
    /// - `Assoc::Before`: the position is "before" the insert,
    ///   so it stays at the original offset.
    /// - `Assoc::After`: the position is "after" the insert,
    ///   so it advances by the insertion length.
    ///
    /// CM6 calls this `bias` / `assoc`. The behavior matters for
    /// cursors at edit boundaries — a typing cursor sits *after*
    /// its insert, a fixed anchor stays *before*.
    #[must_use]
    pub fn map_position(&self, mut pos: usize, assoc: Assoc) -> usize {
        for change in &self.inner {
            let is_insert = change.from == change.to;
            // Sorted: anything after `pos` can't affect us, but a
            // pure insert at exactly `pos` is the boundary case
            // that `assoc` resolves.
            if pos < change.from {
                break;
            }
            if is_insert {
                if pos == change.from {
                    return match assoc {
                        // Anchor stays "before" the inserted text.
                        Assoc::Before => pos,
                        // Typing cursor follows the insertion.
                        Assoc::After => pos + change.inserted.len(),
                    };
                }
                // pos > change.from (we already checked < above).
                pos += change.inserted.len();
                continue;
            }
            // Range edit.
            if pos < change.to {
                // pos lands strictly inside the deletion *or* on
                // its left boundary. Boundary is a real choice —
                // Before keeps the position outside the change,
                // After pushes it to the end of the new content.
                return match assoc {
                    Assoc::Before => change.from,
                    Assoc::After => change.from + change.inserted.len(),
                };
            }
            // pos >= change.to → past the deletion, shift by delta.
            pos = (pos as isize + change.delta()) as usize;
        }
        pos
    }
}

/// Association bias for [`Changes::map_position`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assoc {
    /// Position sticks to what was on its left side before the
    /// change. Use for the *start* of a range that should not
    /// grow when text is inserted at its edge.
    Before,
    /// Position sticks to what was on its right side before the
    /// change — i.e., moves with following text. Use for the
    /// *end* of a range, or for a typing cursor.
    After,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_insert() {
        let d = Doc::from_str("hello");
        let cs = Changes::insert(5, " world");
        assert_eq!(cs.apply(&d).to_string(), "hello world");
    }

    #[test]
    fn apply_delete() {
        let d = Doc::from_str("hello world");
        let cs = Changes::delete(5..6);
        assert_eq!(cs.apply(&d).to_string(), "helloworld");
    }

    #[test]
    fn apply_replace() {
        let d = Doc::from_str("hello world");
        let cs = Changes::replace(6..11, "rust");
        assert_eq!(cs.apply(&d).to_string(), "hello rust");
    }

    #[test]
    fn apply_multiple_in_order() {
        let d = Doc::from_str("hello world");
        let cs = Changes::from_sorted(vec![
            Change::replace(0..5, "HELLO"),
            Change::replace(6..11, "WORLD"),
        ]);
        assert_eq!(cs.apply(&d).to_string(), "HELLO WORLD");
    }

    #[test]
    fn invert_insert_round_trips() {
        let d = Doc::from_str("hello");
        let cs = Changes::insert(5, " world");
        let after = cs.apply(&d);
        let inv = cs.invert(&d);
        assert_eq!(inv, Changes::delete(5..11));
        assert_eq!(inv.apply(&after).to_string(), "hello");
    }

    #[test]
    fn invert_delete_round_trips() {
        let d = Doc::from_str("hello world");
        let cs = Changes::delete(5..11);
        let after = cs.apply(&d);
        let inv = cs.invert(&d);
        assert_eq!(inv, Changes::insert(5, " world"));
        assert_eq!(inv.apply(&after).to_string(), "hello world");
    }

    #[test]
    fn invert_replace_round_trips() {
        let d = Doc::from_str("hello world");
        let cs = Changes::replace(6..11, "rust");
        let after = cs.apply(&d);
        assert_eq!(after.to_string(), "hello rust");
        let inv = cs.invert(&d);
        assert_eq!(inv, Changes::replace(6..10, "world"));
        assert_eq!(inv.apply(&after).to_string(), "hello world");
    }

    #[test]
    fn invert_multi_change_round_trips() {
        let d = Doc::from_str("hello world");
        let cs = Changes::from_sorted(vec![
            Change::replace(0..5, "HI"),
            Change::insert(6, "big "),
            Change::delete(6..11),
        ]);
        let after = cs.apply(&d);
        assert_eq!(after.to_string(), "HI big ");
        let inv = cs.invert(&d);
        assert_eq!(inv.apply(&after).to_string(), "hello world");
    }

    #[test]
    fn invert_utf8_round_trips() {
        let d = Doc::from_str("héllo wörld");
        // Replace the multibyte "ö" (bytes 8..10) with "o".
        let cs = Changes::replace(8..10, "o");
        let after = cs.apply(&d);
        assert_eq!(after.to_string(), "héllo world");
        let inv = cs.invert(&d);
        assert_eq!(inv.apply(&after).to_string(), "héllo wörld");
    }

    #[test]
    fn map_position_past_insert_shifts_forward() {
        // Insert "AB" at offset 2 in "01234".
        let cs = Changes::insert(2, "AB");
        // Position 3 (the '3') becomes 5.
        assert_eq!(cs.map_position(3, Assoc::After), 5);
    }

    #[test]
    fn map_position_at_insert_assoc_before_stays() {
        let cs = Changes::insert(3, "X");
        // Anchor at exactly the insert point with Before bias
        // stays at 3 ("before the insert").
        assert_eq!(cs.map_position(3, Assoc::Before), 3);
    }

    #[test]
    fn map_position_at_insert_assoc_after_advances() {
        let cs = Changes::insert(3, "X");
        // Typing cursor: After bias, moves past the insertion.
        assert_eq!(cs.map_position(3, Assoc::After), 4);
    }

    #[test]
    fn map_position_inside_delete_snaps_to_start() {
        // Delete 2..6 from "abcdefgh".
        let cs = Changes::delete(2..6);
        // Position 4 was inside — collapses to 2.
        assert_eq!(cs.map_position(4, Assoc::Before), 2);
        assert_eq!(cs.map_position(4, Assoc::After), 2);
    }

    #[test]
    fn map_position_past_delete_shifts_back() {
        let cs = Changes::delete(2..6);
        // Position 7 -> 3 (shifted left by 4).
        assert_eq!(cs.map_position(7, Assoc::After), 3);
    }

    #[test]
    fn map_position_before_change_unchanged() {
        let cs = Changes::insert(5, "X");
        assert_eq!(cs.map_position(2, Assoc::After), 2);
    }
}
