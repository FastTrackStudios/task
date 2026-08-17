# Tile Tree Port — Roadmap

Porting the CodeMirror 6 ContentView hierarchy to Rust + Dioxus.
Reference: `~/Development/research/codemirror/view/src/`.

**Scope:** full faithful port of the view tree. Skipping only
bidi (until we need RTL) and heightmap (until we need viewport
virtualization for huge docs). Everything else CM6 does, we do.

**Realistic timeline:** 3-5 focused months of single-developer
work. We commit small, ship each piece with tests, and the
outliner stays paused on the editor until landing.

## Reading list (one-time, before starting)

In order — each builds on the previous:

1. `view/src/contentview.ts` — base class. The vocabulary of
   "dirty," "synced," "posFromDOM," etc. lives here.
2. `view/src/inlineview.ts` — TextView, MarkView, WidgetView.
   Most of the action.
3. `view/src/blockview.ts` — LineView, BlockWidgetView.
4. `view/src/buildtile.ts` — turns a doc + decoration set into
   a tile tree. Bridges state ↔ view.
5. `view/src/docview.ts` — root, plus the high-level sync /
   apply / update loop.
6. `view/src/domobserver.ts` — catches browser mutations,
   handles composition, IME, mobile autocorrect.
7. `view/src/domreader.ts` — text extraction from DOM after a
   mutation we didn't make.
8. `view/src/domchange.ts` — turns observed mutations into
   Transactions.
9. `view/src/cursor.ts` — caret motion (left/right/up/down) in
   the tile tree.

## Phases

### Phase 1 — ContentView trait + child storage

**Goal:** a base trait every tile type implements, holding
`pos`, `length`, `breakAfter`, dirty bits, parent pointer,
children Vec.

- `editor-view/src/contentview/mod.rs` — `ContentView` trait
- `editor-view/src/contentview/dirty.rs` — `Dirty` bitflags
- `editor-view/src/contentview/pos.rs` — posFromDOM /
  posAtPos / nearest

Tests: a hand-built three-tile tree, exercise posFromDOM /
posAtPos for every boundary case.

### Phase 2 — TextView

Leaf for plain text. Owns its text content + corresponding
DOM text node. Implements `merge` (for combining adjacent
TextViews with the same marks) and `slice`.

### Phase 3 — MarkView

Container that wraps children with a mark decoration. Renders
as `<span class="...">`. Implements `merge` so adjacent same-
mark MarkViews fuse into one.

### Phase 4 — WidgetView

Inline widget. Replaces a doc range OR sits between characters.
Owns its widget instance (a trait object: `Widget`). Custom
posAtPos / posFromDOM since DOM content isn't in the doc.

### Phase 5 — Flat docview (no LineView yet)

A DocView that holds inline children directly. Use this to
prove the inline tile tree + decoration sync before adding
block layer.

### Phase 6 — buildtile

Function that consumes a `Doc`, a `DecorationSet`, and an
old tile tree (for diffing), and produces a new tree.

### Phase 7 — DocView, render pass

The component fns: build → sync → patch DOM minimally.
Replaces the flat-Vec render module entirely.

### Phase 8 — LineView + BlockView

Adds block layer. Each line is a tile.

### Phase 9 — DOMObserver

`MutationObserver` wired through dioxus.send. Distinguishes
input mutations from programmatic ones.

### Phase 10 — Composition handling

IME composition crosses multiple mutations. CM6 pauses
observation during composition and rebuilds at the end.

### Phase 11 — Wire into Editor component

Replace the current flat-Vec render + diff_text path with
the tile-tree based one.

### Phase 12 — DOMObserver-driven Changes

domchange.ts: reading a DOM mutation back into a
`Changes` value is the inverse of buildtile. Required for
non-keystroke edits to flow correctly.

## Out of scope (won't port unless we hit the need)

- `bidi.ts` — RTL / mixed-direction text
- `heightmap.ts` — viewport virtualization for huge docs
- `viewstate.ts` — scroll geometry tracking
- `gutter.ts` — line numbers, breakpoints
- `panel.ts` — top/bottom panels
- `tooltip.ts` — hover tooltips
- `rectangular-selection.ts` — Alt-drag column selection
- `dropcursor.ts` — drop indicator
- `lint.ts` — lint message rendering

Most of these are independent features that can be ported
later without touching the tile tree.

## Implementation conventions for the port

- Each TS class → one Rust struct (not a trait) when concrete,
  one trait when it's an abstract base.
- `dirty: Dirty` bitflags replicated as a Rust bitflags type.
- `breakAfter: number` (their newline indicator) → `pub
  break_after: u8`.
- Position math uses byte offsets throughout (CM6 uses UTF-16
  code units; we don't have the browser-Selection constraint
  for the internal model).
- DOM access goes through `document::eval` for now. Future
  work may move to web_sys downcast for the wasm path.
- Tests live in the same module as the code they test, marked
  `#[cfg(test)]`.
