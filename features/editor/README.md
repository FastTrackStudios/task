# Editor

A Rust-native text editor for Dioxus, designed around the architectural ideas
of [CodeMirror 6](https://codemirror.net/) — without being a verbatim port.

## What this is

A small library of crates that provide:

- A pure-Rust document + transaction model (`editor-state`)
- A Dioxus `<Editor>` component backed by `contenteditable` with a Rust-owned
  selection and decoration system (`editor-view`)
- An umbrella crate (`editor`) that re-exports the common surface

## Why CodeMirror 6 as a reference

CM6 is the editor under Obsidian, Replit, JupyterLab, Marimo, and many others.
Its architecture has held up for a reason:

- **Plain-text document, decorations laid on top.** Markdown stays markdown —
  styling, hidden markers, widgets are all overlays, not changes to the
  underlying string. This fits a markdown-file-backed app exactly.
- **Transactions over mutation.** Edits are values you produce and apply, not
  imperative DOM calls. The view is a function of state.
- **Composable extensions.** Decorations, keymaps, and behaviors are
  independent units the user composes; the editor doesn't know markdown or
  vim itself.
- **Position anchors.** Cursors and ranges survive concurrent edits because
  they're tracked through transformation, not snapshotted.

We're not porting CM6. We're taking the same ideas and writing them the
Rust-native way — typed enums where TS uses string tags, ownership where TS
uses shared references, signals where TS uses observers.

## What it's not

- Not a code editor with syntax highlighting (yet)
- Not a port of `lezer` (the incremental parser) — markdown parsing is
  one-shot per block until that becomes a bottleneck
- Not aimed at gigabyte files — block-sized content (≤ a few KB per block)
  is the sweet spot
- Not aimed at non-DOM renderers — `editor-view` targets `dioxus-web` and
  `dioxus-desktop` (webview). `dioxus-native`/blitz would need a different
  view layer.

## Crates

| Crate | What it does |
|---|---|
| `editor-state` | Doc, transactions, selections, decorations, extensions. No DOM. Pure logic + tests. |
| `editor-view`  | `<Editor>` Dioxus component. Renders the doc to a contenteditable, bridges DOM events back to transactions. |
| `editor`       | Umbrella re-export. What downstream apps depend on. |

## CRDT story (planned, not in v1)

The architecture is built so that a CRDT integration can sit *between* the
canonical document and the view. Per-block `LoroText` for content, a tree
CRDT for block structure. The view consumes transactions; whether those
transactions came from local input or a remote peer doesn't matter to it.

## License

MIT.
