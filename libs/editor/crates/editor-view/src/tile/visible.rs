//! Visible-text mirror of the tile tree.
//!
//! When the DOM is read back via `textContent` (or our
//! line-joining `readText`), what we get is **visible** text —
//! the bytes that actually rendered into the DOM. Hidden tiles
//! (zero-width Widget replacements for markdown markers, etc.)
//! contribute zero visible bytes but still occupy doc space.
//! So `visible.len() != doc.len()` whenever any Hidden tile is
//! present.
//!
//! To translate edits applied to visible text back to the doc,
//! we need:
//!
//! - The *exact* visible string the DOM should have right now
//!   (so we can diff against the post-edit `textContent`).
//! - A mapping from visible positions to doc positions, so the
//!   resulting diff offsets land in doc space.
//!
//! [`VisibleText`] holds both. It's computed once per render
//! from the arena.
//!
//! CM6 reference: the equivalent translation lives across
//! `domreader.ts` (text extraction) and `domchange.ts` (Change
//! derivation) — we collapse them here because our diff is
//! simpler (whole-string trim, not per-mutation analysis).

use crate::tile::TileKind;
use crate::tile::arena::{Arena, TileId};
use crate::tile::pos::pos_at_start;
use crate::tile::text::text_of;

/// Visible-space mirror plus a visible↔doc offset map.
#[derive(Clone, Debug, Default)]
pub struct VisibleText {
    /// The string the DOM should have right now (with `\n`
    /// between lines, no bytes from Hidden tiles).
    pub text: String,
    /// Sorted ascending by `visible`. Each entry records that
    /// `visible_pos = visible` corresponds to `doc_pos = doc`.
    /// Lookup: find the largest entry whose `visible <=` your
    /// query, then `doc + (query - entry.visible)`. The map
    /// always starts with `(0, 0)` and gets an extra entry at
    /// every visible↔doc boundary divergence (i.e., at every
    /// Hidden tile start/end).
    pub map: Vec<MapEntry>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MapEntry {
    pub visible: usize,
    pub doc: usize,
}

impl VisibleText {
    /// Walk the arena starting at `root` and accumulate text +
    /// offset map.
    #[must_use]
    pub fn from_arena(arena: &Arena, root: TileId) -> Self {
        let mut out = Self {
            text: String::new(),
            map: vec![MapEntry { visible: 0, doc: 0 }],
        };
        walk(arena, root, &mut out);
        out
    }

    /// Translate a visible-space offset to a doc-space offset.
    ///
    /// Take the **last** entry whose `visible <= visible_pos`.
    /// Combined with [`walk`] never emitting an "after hidden"
    /// entry, this lands a query at a visible boundary INSIDE
    /// the surrounding visible content rather than after a
    /// zero-width hidden range. Concretely for `**bold**` with
    /// `**`s hidden:
    /// - `visible_to_doc(0)` → 2 (start of "b" in doc — after
    ///   leading `**`, inside bold).
    /// - `visible_to_doc(4)` → 6 (just past "d" — before
    ///   trailing `**`, still inside bold).
    ///
    /// User typing at either edge extends the bold span.
    #[must_use]
    pub fn visible_to_doc(&self, visible_pos: usize) -> usize {
        if self.map.is_empty() {
            return visible_pos;
        }
        // partition_point on `e.visible <= visible_pos` gives
        // first index where the predicate becomes false; `-1`
        // is the last index where it was true.
        let i = self.map.partition_point(|e| e.visible <= visible_pos);
        let idx = i.saturating_sub(1);
        let e = self.map[idx];
        e.doc + (visible_pos - e.visible)
    }

    /// Record a mapping at the current visible offset for the
    /// given doc offset. Skips no-op entries (where the entry
    /// would line up perfectly with the previous one — visible
    /// and doc still advancing in lockstep).
    fn push_map(&mut self, doc: usize) {
        let visible = self.text.len();
        if let Some(last) = self.map.last() {
            let expected_doc = last.doc + (visible - last.visible);
            if expected_doc == doc {
                return;
            }
        }
        self.map.push(MapEntry { visible, doc });
    }
}

fn walk(arena: &Arena, tile: TileId, out: &mut VisibleText) {
    let t = arena.get(tile);
    match t.kind {
        TileKind::Text => {
            out.push_map(pos_at_start(arena, tile));
            out.text.push_str(text_of(t));
        }
        TileKind::Widget | TileKind::WidgetBuffer => {
            // Hidden widgets (empty html) contribute 0 visible
            // bytes but t.length doc bytes. Non-empty widgets
            // (real widget content) are also treated as
            // zero-visible in v1 so their inner HTML doesn't
            // get diff'd as user text. Either way `doc` jumps
            // by `t.length` while `visible` stays put — we
            // emit a `(visible, doc_at_start)` entry that's a
            // no-op (since the previous map's lockstep would
            // produce the same `doc`). We intentionally DO NOT
            // emit a `(visible, doc_at_end)` "after" entry.
            // The next non-hidden tile's map entry will record
            // the post-skip doc position; until then,
            // visible_to_doc resolves into the visible content
            // before the hidden range — exactly what the
            // editor-input intuition wants ("typing at the end
            // of a span stays inside the span").
            out.push_map(pos_at_start(arena, tile));
        }
        TileKind::Line => {
            out.push_map(pos_at_start(arena, tile));
            for &child in &t.children {
                walk(arena, child, out);
            }
            if t.break_after() {
                out.text.push('\n');
                // The \n is byte 1 of doc + byte 1 of visible —
                // both advance in lockstep, no map entry needed.
            }
        }
        TileKind::Doc | TileKind::Mark | TileKind::BlockWrapper => {
            for &child in &t.children {
                walk(arena, child, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::build::build_tiles;
    use editor_state::Decoration;

    #[test]
    fn plain_text_visible_matches_doc() {
        let (arena, root) = build_tiles("hello", &[]);
        let v = VisibleText::from_arena(&arena, root);
        assert_eq!(v.text, "hello");
        assert_eq!(v.visible_to_doc(3), 3);
    }

    #[test]
    fn newlines_join_lines() {
        let (arena, root) = build_tiles("ab\ncd", &[]);
        let v = VisibleText::from_arena(&arena, root);
        assert_eq!(v.text, "ab\ncd");
        // Visible 4 = 'd' in the second line, doc 4 = same.
        assert_eq!(v.visible_to_doc(4), 4);
    }

    #[test]
    fn hidden_range_compresses_visible() {
        // doc "**bold**" with replace(0..2) and replace(6..8).
        let decs = vec![Decoration::replace(0..2), Decoration::replace(6..8)];
        let (arena, root) = build_tiles("**bold**", &decs);
        let v = VisibleText::from_arena(&arena, root);
        // Hidden 0..2 + visible "bold" (4 chars) + hidden 6..8.
        assert_eq!(v.text, "bold");
        // visible 0 = 'b' = doc 2. visible_to_doc bridges over
        // the leading hidden range.
        assert_eq!(v.visible_to_doc(0), 2);
        // visible 4 (= end) = doc 6 (just before the trailing
        // hidden range).
        assert_eq!(v.visible_to_doc(4), 6);
    }

    #[test]
    fn mark_does_not_affect_mapping() {
        // Mark decorations don't change visible text or
        // positions — they just wrap children in a span.
        let decs = vec![Decoration::mark(1..4, "bold")];
        let (arena, root) = build_tiles("abcde", &decs);
        let v = VisibleText::from_arena(&arena, root);
        assert_eq!(v.text, "abcde");
        assert_eq!(v.visible_to_doc(2), 2);
    }

    #[test]
    fn map_starts_with_zero_zero() {
        let (arena, root) = build_tiles("", &[]);
        let v = VisibleText::from_arena(&arena, root);
        assert_eq!(v.map[0].visible, 0);
        assert_eq!(v.map[0].doc, 0);
    }
}
