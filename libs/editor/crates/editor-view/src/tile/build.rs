//! `buildtile` — build a tile tree from a `Doc` + decoration
//! set. This is the Rust port of CM6's
//! `view/src/buildtile.ts:39+` (the `TileBuilder` class) +
//! the main `TileUpdate.build` entry point.
//!
//! ## Tree shape (Phase 8: with `LineTiles`)
//!
//! ```text
//! DocTile
//! ├── LineTile (length=5, breakAfter=true)
//! │   ├── TextTile  "he"
//! │   ├── MarkTile (md-bold)
//! │   │   └── TextTile  "llo"
//! ├── LineTile (length=6, breakAfter=false)
//! │   └── TextTile  "world!"
//! ```
//!
//! Each logical line becomes a `LineTile` child of `DocTile`.
//! Inline content (text, marks, widgets, hidden replacements)
//! lives inside the current `LineTile`. The `\n` byte itself
//! is *not* a tile — it's represented by the `BreakAfter` flag
//! on the preceding line, contributing 1 to its
//! `length + break_after` accounting. A trailing `\n` produces
//! a final empty `LineTile`.
//!
//! ## What's NOT here yet, intentionally
//!
//! - **Tile cache / reuse.** CM6's `TileCache` recycles DOM
//!   nodes from the previous build. We rebuild from scratch
//!   each render; Dioxus's reconciler handles DOM reuse.
//! - **Composition handling.** IME composition crosses many
//!   builds and CM6 has explicit "carry forward composition"
//!   logic (`addComposition`). Comes in Phase 10.
//! - **Block wrappers + line decorations.** No block-level
//!   decoration set today.
//! - **Mark stacking algorithm.** CM6's `ensureMarks` opens
//!   and closes mark wrappers as we walk; ours is simpler
//!   because we rebuild the mark stack at every boundary.
//! - **`openStart`/`openEnd` for wrapper carry-over** — only
//!   needed when reusing across builds. We rebuild.
//!
//! Each gap above ports cleanly later without re-architecting.

use editor_state::{DecoratedRange, DecorationKind};

use crate::tile::arena::{Arena, TileId};
use crate::tile::flag::{TileFlag, TileFlagSet};
use crate::tile::line::{new_line_tile, push_line_class};
use crate::tile::mark::{MarkSpec, new_mark_tile};
use crate::tile::text::new_text_tile;
use crate::tile::widget::{is_point_widget, new_widget_buffer_tile, new_widget_tile};
use crate::tile::{Tile, TileBody, TileKind};

/// Build the tile tree for `text` + `decorations`. Returns the
/// fresh arena and the root `DocTile`'s id.
///
/// `decorations` must be sorted by `from` ascending. Within
/// ties: Replace decorations are applied before Mark
/// decorations at the same start offset (so a Replace doesn't
/// get accidentally wrapped in a Mark).
#[must_use]
pub fn build_tiles(text: &str, decorations: &[DecoratedRange]) -> (Arena, TileId) {
    let mut arena = Arena::new();
    let doc_id = arena.insert(Tile {
        parent: None,
        children: Vec::new(),
        length: text.len(),
        kind: TileKind::Doc,
        body: TileBody::Empty,
        flags: TileFlagSet::empty(),
    });

    let mut builder = TileBuilder::new(&mut arena, doc_id);
    builder.run(text, decorations);
    (arena, doc_id)
}

/// Mutable state during a build pass. Ports CM6's
/// `TileBuilder` (`buildtile.ts:39-240`), trimmed to the v1
/// surface.
struct TileBuilder<'a> {
    arena: &'a mut Arena,
    root: TileId,
    /// Current `LineTile` under which inline children are
    /// appended. `None` between lines (e.g. right after a
    /// `\n`) — re-created lazily by [`Self::ensure_line`].
    /// Mirrors CM6's `TileBuilder.curLine` (`buildtile.ts:40`).
    cur_line: Option<TileId>,
    /// Doc byte offset where the next emitted tile starts.
    pos: usize,
}

impl<'a> TileBuilder<'a> {
    fn new(arena: &'a mut Arena, root: TileId) -> Self {
        Self {
            arena,
            root,
            cur_line: None,
            pos: 0,
        }
    }

    /// Lazily create the current `LineTile`, attaching it to
    /// the Doc. Called from every emit_* method — guarantees
    /// inline content always has a line to live in.
    fn ensure_line(&mut self) -> TileId {
        if let Some(id) = self.cur_line {
            return id;
        }
        let line_id = self.arena.insert(new_line_tile());
        self.arena.get_mut(line_id).parent = Some(self.root);
        self.arena.get_mut(self.root).children.push(line_id);
        self.cur_line = Some(line_id);
        line_id
    }

    /// Close the current `LineTile` by marking it `BreakAfter`.
    /// Next emit will start a fresh line via `ensure_line`.
    /// Mirrors `TileBuilder.endLine` (`buildtile.ts:125`) — the
    /// observable effect, anyway; CM6 also flushes widget
    /// buffers and emits any line decorations, which we'll
    /// fold in once those land.
    fn end_line(&mut self) {
        let line_id = self.ensure_line();
        self.arena
            .get_mut(line_id)
            .flags
            .insert(TileFlag::BreakAfter);
        self.cur_line = None;
    }

    /// Main loop. Walks `text` + `decorations` together and
    /// emits tiles. Single pass, O(n + d log d) once decorations
    /// are sorted.
    fn run(&mut self, text: &str, decorations: &[DecoratedRange]) {
        // Collect "events" — decoration boundaries — sorted by
        // position. For each event we know what changes at that
        // offset (a mark starts, ends, a replace covers a range,
        // a widget appears at a point).
        //
        // We walk from one event to the next, emitting plain
        // text in between under the active mark stack.
        let mut events = collect_events(decorations);
        events.sort_by_key(|e| e.at);

        // Decorations active right now. We rebuild the mark
        // stack each segment from this set; cheap because the
        // set is tiny (one or two marks in practice).
        let mut active_marks: Vec<MarkSpec> = Vec::new();
        let mut active_replace_until: Option<usize> = None;

        let mut event_idx = 0;
        while self.pos < text.len() || event_idx < events.len() {
            // Consume events that fall *behind* the current pos —
            // typically Mark{Start,End} pairs whose range was
            // (partially) covered by a Replace/Widget that we
            // already walked past. Without this, the next
            // iteration's `next_event_pos < self.pos` traps the
            // loop in an infinite no-op spin.
            //
            // Mark events must still update `active_marks` even
            // when skipped: a mark that ends inside a consumed
            // range would otherwise stay open forever (e.g. the
            // vim block caret `0..1` inside hidden YAML
            // frontmatter `0..N` used to wrap the ENTIRE rest of
            // the doc in `ed-modal-caret-block`). Applying both
            // start and end keeps the bookkeeping exact: a mark
            // fully inside the hidden range nets to zero (paints
            // nothing), one that extends past it stays active and
            // paints only its visible slice.
            while event_idx < events.len() && events[event_idx].at < self.pos {
                match &events[event_idx].kind {
                    EventKind::MarkStart(spec) => active_marks.push(spec.clone()),
                    EventKind::MarkEnd(spec) => {
                        if let Some(p) = active_marks.iter().rposition(|m| m == spec) {
                            active_marks.remove(p);
                        }
                    }
                    _ => {}
                }
                event_idx += 1;
            }
            // Apply all events whose position == self.pos. A
            // Replace event opens a hidden range until its
            // `to`; a Mark event opens a mark wrapper for its
            // span.
            while event_idx < events.len() && events[event_idx].at == self.pos {
                let ev = &events[event_idx];
                event_idx += 1;
                match ev.kind {
                    EventKind::MarkStart(ref spec) => active_marks.push(spec.clone()),
                    EventKind::MarkEnd(ref spec) => {
                        if let Some(p) = active_marks.iter().rposition(|m| m == spec) {
                            active_marks.remove(p);
                        }
                    }
                    EventKind::ReplaceStart { to } => {
                        active_replace_until = Some(to);
                    }
                    EventKind::ReplaceEnd => {
                        active_replace_until = None;
                    }
                    EventKind::Widget { length, ref html } => {
                        // Decoration widgets carry no side bits today, so
                        // point widgets default to side 0 → buffered on
                        // both sides (CM6's behavior for a neutral inline
                        // widget). The `flags` arg is the wiring point for
                        // a future `side` option on `Decoration::widget`.
                        self.emit_widget(html, length, TileFlagSet::empty(), &active_marks);
                    }
                    EventKind::Line(ref class) => {
                        let line_id = self.ensure_line();
                        push_line_class(self.arena.get_mut(line_id), class);
                    }
                }
            }

            // What's the next event position? Cap any emitted
            // segment so it ends at the next event (and we
            // re-enter the inner loop above).
            let next_event_pos = events.get(event_idx).map_or(text.len(), |e| e.at);

            if let Some(replace_until) = active_replace_until {
                // We're inside a Replace decoration: emit one
                // hidden widget covering this run, advance pos
                // to the replace's end (or the next event,
                // whichever is smaller — overlapping replaces
                // we resolve as "first wins").
                let until = replace_until.min(text.len());
                if until > self.pos {
                    self.emit_hidden(until - self.pos);
                }
                // Once we've reached replace_until, close it
                // out so the outer loop picks up the
                // ReplaceEnd event.
                if self.pos >= replace_until {
                    active_replace_until = None;
                }
                continue;
            }

            // Plain text run from self.pos to next_event_pos,
            // split at `\n` boundaries (lines are still a single
            // text node in v1 — Phase 8 will introduce LineTile).
            let end = next_event_pos.min(text.len());
            if end > self.pos {
                self.emit_text_run(&text[self.pos..end], &active_marks);
            } else if event_idx >= events.len() {
                // No events left and we're at end of text — done.
                break;
            }
        }
        // Ensure at least one LineTile exists, and that the
        // doc *ends* with a non-breaking LineTile (so a
        // trailing `\n` or an empty doc still has a place for
        // the caret to land). CM6 always renders at least one
        // line; without this our contenteditable would
        // sometimes have zero `<div class="cm-line">` children.
        self.ensure_line();
    }

    /// Emit text spanning `slice` under the current mark stack.
    /// On every `\n` we [`end_line`], so the next emit starts a
    /// fresh `LineTile`. CM6's analogous flow lives in
    /// `buildtile.ts` where text is broken at line boundaries
    /// by the calling iteration loop.
    fn emit_text_run(&mut self, slice: &str, marks: &[MarkSpec]) {
        let mut local = 0;
        for (i, ch) in slice.char_indices() {
            if ch == '\n' {
                if i > local {
                    self.emit_text(&slice[local..i], marks);
                }
                // Newline closes the current LineTile.
                // `pos` advances by 1 to account for the \n in
                // doc bytes, but no tile owns the byte —
                // `BreakAfter` on the line stores the fact.
                self.end_line();
                self.pos += 1;
                local = i + 1;
            }
        }
        if local < slice.len() {
            self.emit_text(&slice[local..], marks);
        }
    }

    /// Emit one `TextTile` under the current mark stack. Wraps
    /// the text in `MarkTile`s outermost-first to match the
    /// CSS class layering you'd expect (`<span class="bold">
    /// <span class="italic">…</span></span>`).
    fn emit_text(&mut self, text: &str, marks: &[MarkSpec]) {
        let text_tile = self.insert_under_marks(marks, |arena| arena.insert(new_text_tile(text)));
        let len = text.len();
        // Bubble length up the parent chain.
        self.bump_lengths_up(text_tile, len);
        self.pos += len;
    }

    /// Emit a widget under the current mark stack.
    ///
    /// Replace widgets (`length > 0`) cover doc bytes and render as a
    /// single span — no buffers, the cursor never sits directly beside
    /// the uneditable node the way it does next to a zero-width point
    /// widget.
    ///
    /// Point widgets (`length == 0`) get bracketed with
    /// [`WidgetBufferTile`]s — zero-width `<img>` anchors that give the
    /// browser an editable-side stop on each side of the uneditable
    /// node, working around the family of caret bugs that show up when
    /// the cursor is directly next to `contenteditable=false` inline
    /// content. 1:1 with CM6's `addInlineWidget` + `flushBuffer`
    /// (`buildtile.ts:100-126, 232-237`): a leading buffer unless the
    /// widget faces `Before`, a trailing buffer unless it faces `After`,
    /// and the leading buffer is skipped when the previous sibling is
    /// already a buffer or another point widget (`noSpace`).
    ///
    /// [`WidgetBufferTile`]: crate::tile::widget::new_widget_buffer_tile
    fn emit_widget(&mut self, html: &str, length: usize, flags: TileFlagSet, marks: &[MarkSpec]) {
        if length > 0 {
            let widget_tile = self.insert_under_marks(marks, |arena| {
                arena.insert(new_widget_tile(html, length, flags))
            });
            self.bump_lengths_up(widget_tile, length);
            self.pos += length;
            return;
        }

        let parent = self.ensure_marks(marks);
        // Leading buffer: skip when the widget explicitly faces the
        // position before it, or when we'd be stacking it against an
        // adjacent buffer / point widget.
        if !flags.contains(TileFlag::Before) && !self.last_child_is_buffer_zone(parent) {
            let buf = self
                .arena
                .insert(new_widget_buffer_tile(flag(TileFlag::After)));
            self.append_to(parent, buf);
        }
        let widget = self.arena.insert(new_widget_tile(html, 0, flags));
        self.append_to(parent, widget);
        // Trailing buffer: skip when the widget faces the position after
        // it (a following editable char already provides the anchor).
        if !flags.contains(TileFlag::After) {
            let buf = self
                .arena
                .insert(new_widget_buffer_tile(flag(TileFlag::Before)));
            self.append_to(parent, buf);
        }
        // Zero length: nothing to bump, pos unchanged.
    }

    /// `true` when `parent`'s last child is a widget buffer or another
    /// point widget — i.e. we're already in a buffered zone and a fresh
    /// leading buffer would just be redundant DOM. Mirrors CM6's
    /// `noSpace` check in `addInlineWidget` (`buildtile.ts:101-103`).
    fn last_child_is_buffer_zone(&self, parent: TileId) -> bool {
        self.arena.get(parent).children.last().is_some_and(|&id| {
            let t = self.arena.get(id);
            matches!(t.kind, TileKind::WidgetBuffer)
                || (matches!(t.kind, TileKind::Widget) && is_point_widget(t))
        })
    }

    /// Emit a hidden replacement. Length covers the replaced
    /// doc bytes; no DOM is produced (renderer skips Widget
    /// tiles with empty html and no widget content).
    fn emit_hidden(&mut self, length: usize) {
        let line_id = self.ensure_line();
        let mut flags = TileFlagSet::empty();
        // Mark with both IncStart + IncEnd so adjacent typing
        // doesn't accidentally extend into the hidden range.
        flags.insert(TileFlag::IncStart);
        flags.insert(TileFlag::IncEnd);
        let hidden = self.arena.insert(new_widget_tile("", length, flags));
        self.append_to(line_id, hidden);
        self.bump_lengths_up(hidden, length);
        self.pos += length;
    }

    /// Walk the mark stack from outermost to innermost, finding (or
    /// creating) a `MarkTile` of each spec under the previous one.
    /// Returns the inner-most parent into which a caller should insert
    /// their text/widget tile(s). Always rooted in the current
    /// `LineTile`. Mirrors CM6's `ensureMarks` open-stack logic
    /// (`buildtile.ts:160-170`), reduced to a same-mark lookback.
    fn ensure_marks(&mut self, marks: &[MarkSpec]) -> TileId {
        let mut parent = self.ensure_line();
        for spec in marks {
            // Try to reuse the parent's last child if it's the same
            // mark; otherwise open a fresh wrapper.
            let reuse = self
                .arena
                .get(parent)
                .children
                .last()
                .copied()
                .filter(|&id| match &self.arena.get(id).body {
                    TileBody::Mark { spec: existing } => existing == spec,
                    _ => false,
                });
            parent = if let Some(id) = reuse {
                id
            } else {
                let mark_id = self.arena.insert(new_mark_tile(spec.clone()));
                self.append_to(parent, mark_id);
                mark_id
            };
        }
        parent
    }

    /// Ensure the mark chain, then insert a single leaf under the
    /// inner-most `MarkTile`. Returns the leaf's id.
    fn insert_under_marks(
        &mut self,
        marks: &[MarkSpec],
        emit_leaf: impl FnOnce(&mut Arena) -> TileId,
    ) -> TileId {
        let parent = self.ensure_marks(marks);
        let leaf = emit_leaf(self.arena);
        self.append_to(parent, leaf);
        leaf
    }

    /// Append `child` as a child of `parent`, wiring parent
    /// pointer.
    fn append_to(&mut self, parent: TileId, child: TileId) {
        self.arena.get_mut(child).parent = Some(parent);
        self.arena.get_mut(parent).children.push(child);
    }

    /// Walk up from `tile` adding `length` to each ancestor's
    /// length. Stops at the root (`DocTile`) because we set its
    /// length once at the top of `build_tiles` from the full
    /// `text.len()` — bumping it again would double-count.
    /// (Intermediate composites like `MarkTile` still need this
    /// to sum their children correctly.)
    fn bump_lengths_up(&mut self, tile: TileId, length: usize) {
        let mut cur = self.arena.get(tile).parent;
        while let Some(p) = cur {
            if p == self.root {
                break;
            }
            self.arena.get_mut(p).length += length;
            cur = self.arena.get(p).parent;
        }
    }
}

/// Build a single-flag [`TileFlagSet`]. Convenience for the buffer
/// constructors, which take a set but only ever carry one side bit.
fn flag(f: TileFlag) -> TileFlagSet {
    let mut s = TileFlagSet::empty();
    s.insert(f);
    s
}

/// One thing that happens at a doc offset. The builder walks
/// these in order; everything in between events is plain text
/// under the current mark stack.
#[derive(Debug, Clone)]
struct Event {
    at: usize,
    kind: EventKind,
}

#[derive(Debug, Clone)]
enum EventKind {
    MarkStart(MarkSpec),
    MarkEnd(MarkSpec),
    ReplaceStart { to: usize },
    ReplaceEnd,
    Widget { length: usize, html: String },
    Line(String),
}

/// Turn a decoration set into start/end events.
fn collect_events(decorations: &[DecoratedRange]) -> Vec<Event> {
    let mut out = Vec::new();
    for d in decorations {
        match &d.kind {
            DecorationKind::Mark { class, attrs } => {
                let spec = MarkSpec::span_class(class).with_attrs(attrs.clone());
                out.push(Event {
                    at: d.from,
                    kind: EventKind::MarkStart(spec.clone()),
                });
                out.push(Event {
                    at: d.to,
                    kind: EventKind::MarkEnd(spec),
                });
            }
            DecorationKind::Replace => {
                out.push(Event {
                    at: d.from,
                    kind: EventKind::ReplaceStart { to: d.to },
                });
                out.push(Event {
                    at: d.to,
                    kind: EventKind::ReplaceEnd,
                });
            }
            DecorationKind::Widget { html } => {
                out.push(Event {
                    at: d.from,
                    kind: EventKind::Widget {
                        length: d.to - d.from,
                        html: html.clone(),
                    },
                });
            }
            DecorationKind::Line { class } => {
                out.push(Event {
                    at: d.from,
                    kind: EventKind::Line(class.clone()),
                });
            }
            DecorationKind::Atomic => {
                // Atomic is a behavior-only marker — consumed
                // by `decoration::skip_atomic` at the view layer
                // (selection placement) and ignored by the tile
                // builder. No DOM effect.
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_state::Decoration;

    fn children_kinds(arena: &Arena, parent: TileId) -> Vec<TileKind> {
        arena
            .get(parent)
            .children
            .iter()
            .map(|&c| arena.get(c).kind.clone())
            .collect()
    }

    #[test]
    fn empty_text_no_decorations_makes_one_empty_line() {
        let (arena, doc) = build_tiles("", &[]);
        // Doc has at least one LineTile so the caret has
        // somewhere to land.
        assert_eq!(children_kinds(&arena, doc), vec![TileKind::Line]);
        let line = arena.get(doc).children[0];
        assert_eq!(arena.get(line).length, 0);
        assert_eq!(arena.get(doc).length, 0);
    }

    #[test]
    fn plain_text_one_line_with_text() {
        let (arena, doc) = build_tiles("hello", &[]);
        assert_eq!(children_kinds(&arena, doc), vec![TileKind::Line]);
        let line = arena.get(doc).children[0];
        assert_eq!(arena.get(line).length, 5);
        assert_eq!(children_kinds(&arena, line), vec![TileKind::Text]);
        let text = arena.get(line).children[0];
        assert_eq!(arena.get(text).length, 5);
    }

    #[test]
    fn one_mark_wraps_text_inside_line() {
        let decs = vec![Decoration::mark(2..4, "bold")];
        let (arena, doc) = build_tiles("hello", &decs);
        let line = arena.get(doc).children[0];
        let line_kids = children_kinds(&arena, line);
        // Line contains: Text, Mark, Text
        assert_eq!(
            line_kids,
            vec![TileKind::Text, TileKind::Mark, TileKind::Text]
        );
        let mark = arena.get(line).children[1];
        assert_eq!(arena.get(mark).length, 2);
        assert_eq!(arena.get(mark).children.len(), 1);
    }

    #[test]
    fn replace_emits_hidden_widget() {
        let decs = vec![Decoration::replace(0..2)];
        let (arena, doc) = build_tiles("**bold**", &decs);
        let line = arena.get(doc).children[0];
        let first_inline = arena.get(line).children[0];
        assert!(matches!(arena.get(first_inline).kind, TileKind::Widget));
        assert_eq!(arena.get(first_inline).length, 2);
    }

    #[test]
    fn doc_length_sums_lines() {
        let decs = vec![Decoration::mark(2..4, "bold")];
        let (arena, doc) = build_tiles("hello", &decs);
        assert_eq!(arena.get(doc).length, 5);
        let line = arena.get(doc).children[0];
        assert_eq!(arena.get(line).length, 5);
    }

    #[test]
    fn newline_splits_into_two_line_tiles() {
        let (arena, doc) = build_tiles("ab\ncd", &[]);
        let kids = children_kinds(&arena, doc);
        assert_eq!(kids, vec![TileKind::Line, TileKind::Line]);
        let l1 = arena.get(doc).children[0];
        let l2 = arena.get(doc).children[1];
        assert_eq!(arena.get(l1).length, 2);
        assert_eq!(arena.get(l2).length, 2);
        assert!(arena.get(l1).break_after());
        assert!(!arena.get(l2).break_after());
        // l1 length(2) + break(1) + l2 length(2) = 5 ✓
        assert_eq!(arena.get(doc).length, 5);
    }

    #[test]
    fn trailing_newline_produces_empty_final_line() {
        let (arena, doc) = build_tiles("ab\n", &[]);
        let kids = children_kinds(&arena, doc);
        assert_eq!(kids, vec![TileKind::Line, TileKind::Line]);
        let l2 = arena.get(doc).children[1];
        assert_eq!(arena.get(l2).length, 0);
        assert!(!arena.get(l2).break_after());
    }

    #[test]
    fn line_decoration_attaches_class_to_line() {
        let decs = vec![Decoration::line(0, "md-h1")];
        let (arena, doc) = build_tiles("# title", &decs);
        let line = arena.get(doc).children[0];
        assert_eq!(
            crate::tile::line::line_extra_classes(arena.get(line)),
            &["md-h1".to_string()]
        );
    }

    #[test]
    fn multiple_line_decorations_concatenate() {
        // Second line gets two classes; first line stays bare.
        let decs = vec![Decoration::line(3, "md-h1"), Decoration::line(3, "active")];
        let (arena, doc) = build_tiles("ab\ncd", &decs);
        let l1 = arena.get(doc).children[0];
        let l2 = arena.get(doc).children[1];
        assert!(crate::tile::line::line_extra_classes(arena.get(l1)).is_empty());
        assert_eq!(
            crate::tile::line::line_extra_classes(arena.get(l2)),
            &["md-h1".to_string(), "active".to_string()]
        );
    }

    #[test]
    fn point_widget_gets_buffers_on_both_sides() {
        // A zero-width inline widget is bracketed by WidgetBuffer
        // tiles so the caret has an editable-side anchor next to the
        // uneditable node (CM6's `addInlineWidget` + `flushBuffer`).
        let decs = vec![Decoration::widget(2, "<span>x</span>")];
        let (arena, doc) = build_tiles("abcd", &decs);
        let line = arena.get(doc).children[0];
        assert_eq!(
            children_kinds(&arena, line),
            vec![
                TileKind::Text,         // "ab"
                TileKind::WidgetBuffer, // leading buffer
                TileKind::Widget,       // the point widget
                TileKind::WidgetBuffer, // trailing buffer
                TileKind::Text,         // "cd"
            ]
        );
        // Buffers and the widget are all zero doc-length: the line is
        // still just "abcd".
        assert_eq!(arena.get(line).length, 4);
    }

    #[test]
    fn adjacent_point_widgets_dedupe_inner_buffer() {
        // Two widgets at the same offset: buffer, w1, buffer, w2,
        // buffer — the second widget's leading buffer is skipped
        // because it'd stack against the first's trailing buffer
        // (CM6's `noSpace`).
        let decs = vec![
            Decoration::widget(2, "<span>1</span>"),
            Decoration::widget(2, "<span>2</span>"),
        ];
        let (arena, doc) = build_tiles("abcd", &decs);
        let line = arena.get(doc).children[0];
        assert_eq!(
            children_kinds(&arena, line),
            vec![
                TileKind::Text,
                TileKind::WidgetBuffer,
                TileKind::Widget,
                TileKind::WidgetBuffer,
                TileKind::Widget,
                TileKind::WidgetBuffer,
                TileKind::Text,
            ]
        );
    }

    #[test]
    fn replace_widget_gets_no_buffers() {
        // Length-bearing (replace) widgets cover doc bytes and render
        // as a lone span — no buffers.
        let decs = vec![Decoration::replace(0..2)];
        let (arena, doc) = build_tiles("abcd", &decs);
        let line = arena.get(doc).children[0];
        assert_eq!(
            children_kinds(&arena, line),
            vec![TileKind::Widget, TileKind::Text]
        );
    }

    /// Collect every Mark tile in the tree with its class + length.
    fn collect_marks(arena: &Arena, root: TileId) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let t = arena.get(id);
            if let TileBody::Mark { spec } = &t.body {
                out.push((spec.class.clone(), t.length));
            }
            stack.extend(t.children.iter().copied());
        }
        out
    }

    #[test]
    fn mark_fully_inside_replace_paints_nothing() {
        // Regression: the vim block caret is a 1-char mark. When the
        // caret sits at 0 inside hidden YAML frontmatter (a Replace
        // covering 0..N), the mark's End event fell behind `pos` after
        // the replace was consumed and was dropped without popping —
        // the mark stayed active and wrapped the ENTIRE rest of the
        // doc. It must paint nothing (its whole range is hidden).
        for decs in [
            // Both orders at from==0: pipeline sort is stable on `from`,
            // so real inputs can arrive either way.
            vec![
                Decoration::replace(0..8),
                Decoration::mark(0..1, "ed-modal-caret-block"),
            ],
            vec![
                Decoration::mark(0..1, "ed-modal-caret-block"),
                Decoration::replace(0..8),
            ],
        ] {
            let (arena, doc) = build_tiles("---\nx---\nhello", &decs);
            assert_eq!(
                collect_marks(&arena, doc),
                Vec::<(String, usize)>::new(),
                "a mark fully inside a hidden range must render no Mark tile"
            );
        }
    }

    #[test]
    fn mark_fully_inside_replace_widget_paints_nothing() {
        // Same as above but the hidden range is a length-bearing
        // Widget (the markdown frontmatter property table is one).
        let decs = vec![
            DecoratedRange {
                from: 0,
                to: 8,
                kind: DecorationKind::Widget {
                    html: "<table/>".into(),
                },
            },
            Decoration::mark(0..1, "ed-modal-caret-block"),
        ];
        let (arena, doc) = build_tiles("---\nx---\nhello", &decs);
        assert_eq!(
            collect_marks(&arena, doc),
            Vec::<(String, usize)>::new(),
            "a mark fully inside a replace widget must render no Mark tile"
        );
    }

    #[test]
    fn mark_starting_inside_replace_paints_only_visible_tail() {
        // A mark that starts inside a hidden range but extends past it
        // paints only its visible slice.
        let decs = vec![Decoration::replace(0..5), Decoration::mark(2..8, "sel")];
        let (arena, doc) = build_tiles("0123456789", &decs);
        assert_eq!(
            collect_marks(&arena, doc),
            vec![("sel".to_string(), 3)], // bytes 5..8
        );
    }

    #[test]
    fn mark_spanning_hidden_range_paints_visible_slices_only() {
        // vim ggVG on a doc with hidden markers: the selection mark
        // legitimately crosses the hidden range; the hidden bytes are
        // not wrapped, the visible slices on both sides are.
        let decs = vec![Decoration::mark(0..10, "sel"), Decoration::replace(3..6)];
        let (arena, doc) = build_tiles("0123456789", &decs);
        let line = arena.get(doc).children[0];
        assert_eq!(
            children_kinds(&arena, line),
            vec![TileKind::Mark, TileKind::Widget, TileKind::Mark]
        );
        let mut marks = collect_marks(&arena, doc);
        marks.sort();
        assert_eq!(marks, vec![("sel".to_string(), 3), ("sel".to_string(), 4)]);
    }

    #[test]
    fn nested_marks_share_parent_when_same_class() {
        // Two adjacent bold ranges produce ONE MarkTile with
        // both texts as children (CM6's same-mark merging).
        let decs = vec![
            Decoration::mark(0..2, "bold"),
            Decoration::mark(2..4, "bold"),
        ];
        let (arena, doc) = build_tiles("abcd", &decs);
        let line = arena.get(doc).children[0];
        let kids = children_kinds(&arena, line);
        assert_eq!(kids, vec![TileKind::Mark]);
        let mark = arena.get(line).children[0];
        assert_eq!(arena.get(mark).length, 4);
        assert_eq!(arena.get(mark).children.len(), 2);
    }
}
