# Design — CM6 concepts mapped to Rust

The goal of this doc: name each CM6 idea we're keeping, what it does, and how
it shows up in our crates. CM6 reference at `~/Development/research/codemirror/`.

## Vocabulary

| CM6 term | Our equivalent | Lives in |
|---|---|---|
| `EditorState` | `EditorState` (immutable snapshot) | `editor-state` |
| `Transaction` | `Transaction` (a value, not a method call) | `editor-state` |
| `ChangeSet` | `Changes` (list of typed `Change`s) | `editor-state` |
| `SelectionRange` / `EditorSelection` | `Selection` (one or more `Range`s) | `editor-state` |
| `Facet` / `Extension` | `Extension` (typed config plug) | `editor-state` |
| `Decoration` (`mark`/`replace`/`widget`/`line`) | `Decoration` enum | `editor-state` |
| `ViewPlugin` | `ViewPlugin` trait | `editor-view` |
| `EditorView` | `<Editor>` Dioxus component | `editor-view` |
| `keymap`, `keyBinding` | `Keymap` extension | `editor-state` |

## Core principles we're keeping

### 1. The document is plain text. Style is overlay.

CM6 stores a string. Bold isn't a node in a tree — it's a `MarkDecoration`
that says "characters 5..10 get class `bold`." Hidden markers (`**`) are a
`ReplaceDecoration` that removes the range from the rendered view without
removing it from the document.

This is the right model for a **markdown-file-backed** app: the file on disk
is the document, full stop. Live preview is what the *view* shows on top.

### 2. State is immutable. Edits are transactions.

```rust
let tr = state.update(Transaction {
    changes: Changes::insert(5, "x"),
    selection: Some(Selection::single(6)),
    ..Default::default()
});
let new_state = tr.apply();
```

Every edit is a value you can inspect, log, or send over the network. CRDT
integration becomes "translate incoming op → transaction" rather than
patching imperative state.

### 3. Selections are anchors, not snapshots.

A cursor at offset 7 in a doc of length 10 isn't just `7`. It's an anchor
that maps forward through subsequent edits. If someone inserts 3 chars at
offset 2, the cursor becomes 10. CM6 calls this "mapping through changes";
we'll call it the same thing. Without this, CRDT collab is impossible.

### 4. The view is a function of state.

The Dioxus `<Editor>` reads `EditorState` and renders. When state changes
(via a transaction), the view re-renders. There's no `editor.setText()`,
`editor.setSelection()`, etc. — those are all transactions.

### 5. Extensions compose.

You build an editor by handing it a list of extensions:

```rust
Editor::new(EditorState::new("hello", [
    keymap(default_keymap()),
    markdown_live_preview(),
    history(),
]))
```

Each extension contributes facets, view plugins, decorations, or commands.
They don't see each other; the editor merges their outputs.

## What we're *not* doing (yet)

- **Lezer.** CM6's incremental parser is brilliant for code. For block-sized
  markdown, a one-shot parse-on-edit is fine. If we ever need it we'll
  revisit.
- **Height map / viewport virtualization.** Blocks are short. The whole
  block fits in the DOM.
- **Bidi shaping at the editor level.** Browser handles it inside the
  contenteditable.
- **Multi-cursor.** Out for v1; the `Selection` type allows multiple
  ranges so we can add it later without churn.

## Crate layout

```
editor-state/   ← pure logic, no Dioxus, no DOM
  src/
    doc.rs          ← Doc (currently Rope wrapper)
    change.rs       ← Change, Changes, mapping
    selection.rs    ← Range, Selection, anchors
    transaction.rs  ← Transaction, TransactionSpec
    decoration.rs   ← Decoration enum + RangeSet
    extension.rs    ← Extension trait, Facet
    state.rs        ← EditorState
    lib.rs          ← re-exports

editor-view/    ← Dioxus, depends on editor-state
  src/
    editor.rs       ← <Editor> component
    dom_bridge.rs   ← contenteditable selection ↔ Rust selection
    render.rs       ← state + decorations → Dioxus VNodes
    keys.rs         ← keyboard event → command dispatch
    lib.rs

editor/         ← umbrella
  src/lib.rs   ← pub use editor_state::*; pub use editor_view::*;
```

## v1 milestones

1. **`editor-state` skeleton.** `Doc`, `Change`, `Changes::map_position`,
   `Selection`, `Transaction`, `EditorState`. No view. Heavy unit tests for
   position mapping (the foundation everything else depends on).
2. **Minimal `<Editor>`.** Plain text only, contenteditable, types
   propagate, cursor preserved through re-renders. No decorations yet.
3. **Decorations.** Pipe `MarkDecoration` through the render path so a
   range can show up bold. Manual decoration source (extensions come later).
4. **Markdown live-preview extension.** First real extension: parses block
   content, emits `MarkDecoration` for spans, `ReplaceDecoration` for
   marker characters when cursor isn't on the span.

Each step ships independently — `editor-state` can be tested and depended
on long before the view stabilizes.
