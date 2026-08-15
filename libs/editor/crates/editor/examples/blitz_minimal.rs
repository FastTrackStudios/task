//! Full Blitz-native editor demo — the whole embedding story in one file,
//! seeded with a document that exercises every live-preview style (the
//! native counterpart of the web `playground`).
//!
//! Run from the repo root:
//!
//! ```sh
//! cargo run -p editor --example blitz_minimal --no-default-features --features native
//! ```
//!
//! Opens a real `dioxus-native` (Blitz / Vello / wgpu) window — no webview,
//! no JS — with the full markdown editor: live-preview decorations
//! (headings, bold/italic/code/highlight, links & tags, blockquotes,
//! lists, task checkboxes, callouts, tables, code blocks, math), a true
//! reverse-video vim block caret, visual-mode selection (styled + gap-free
//! multi-line), list-continuing Enter, bracket auto-pairing, and word-wise
//! Ctrl-motions — all driven by the same shared `editor_state` core the
//! web build uses, and covered by the headless `dioxus-test` suites in
//! `editor-view/tests/native_*.rs`.
//!
//! Styling is inlined (`dangerous_inner_html`) because Blitz does not
//! load external stylesheets — same rule as the signal-domain UIs.

use dioxus::prelude::*;
use editor::{EditorState, combined_decorations, editor_view, standard_markdown_keymap};
use editor_view::DecorationSource;

/// Full showcase document — exercises every live-preview style so the
/// native demo matches the web playground. Markers hide/reveal as the
/// caret enters each line.
const SEED: &str = "\
# Editor on Blitz

This window is **pure Rust** — Blitz DOM, Vello, wgpu. No webview. It runs
the same `editor_state` core and live-preview pipeline as the web build.

## Inline styles

**bold**, *italic*, ***bold italic***, ~~strikethrough~~, ==highlight==,
`inline code`, and a footnote reference[^1].

Links: [Anthropic](https://anthropic.com), an autolink <https://obsidian.md>,
wikilinks [[Editor Roadmap]] and [[Project README|the readme]], tags
#editor #live-preview #notes/howto.

## Headings

# Heading 1
## Heading 2
### Heading 3
#### Heading 4

## Block styles

> Blockquotes are just blockquotes.
> Multi-line works too.

- Unordered list item
- Another item, with `code` inside
  - nested bullet

1. Ordered list
2. Stays numbered on Enter

- [ ] Click the checkbox to toggle
- [x] Done
- [/] In progress

### Callouts

> [!note] Note
> Callouts share the blockquote syntax.

> [!tip] Tip
> Press `/` anywhere to open the slash-command menu.

> [!warning] Warning
> High-stakes call-out style.

### Table

| Feature     | Status        | Notes                    |
|-------------|---------------|--------------------------|
| Headings    | Mod-1..6      | Mod-0 strips             |
| Tables      | GFM pipe form | rendered inline          |
| Vim         | default-on    | operators, text objects  |
| Selection   | visual mode   | styled + multi-line      |

### Code block

```rust
fn main() {
    println!(\"pure-Rust editor on Blitz\");
}
```

### Math

Inline math via Typst: $E = m c^2$, and a display equation:

$$ sum_(i=1)^n i = n(n+1)/2 $$

---

Try it: vim is on (`hjkl`, `dd`, `ciw`, `vi(`, `v` to select), Enter
continues lists, brackets auto-pair, and `Tab` indents list items.

[^1]: footnotes render as markers you can hover.
";

fn main() {
    // `RUST_LOG=editor_view=debug` shows the keydown dispatch trace.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        // stderr: unbuffered, so traces reach a piped log immediately.
        .with_writer(std::io::stderr)
        .init();
    dioxus_native::launch(App);
}

/// Readable light theme. These are the EXACT token names the editor CSS
/// reads (`--background`, `--foreground`, `--primary`, `--muted`,
/// `--muted-foreground`) — not the architect-ui aliases. A saturated
/// `--primary` matters: the vim block caret paints the glyph under it in
/// `--background` (reverse video), so a light accent makes the character
/// vanish (white-on-light). Deep blue keeps the glyph legible.
const THEME: &str = "
:root {
    --background: #ffffff;
    --muted: #f2f4f7;
    --foreground: #1a1c20;
    --muted-foreground: #6b7280;
    --primary: #1d4ed8;
}
body { background: #ffffff; color: #1a1c20; }
";

#[component]
fn App() -> Element {
    let state = use_signal(|| EditorState::new(SEED));
    let keymap = use_hook(standard_markdown_keymap);
    let vim = use_signal(editor::editor_vim::VimState::new);

    rsx! {
        style { dangerous_inner_html: include_str!("../assets/editor.css") }
        // The editor's CSS reads design-system tokens (--background,
        // --text, --accent, …) that a host app normally injects. This
        // standalone example has no design system, so define a readable
        // light theme here — otherwise text is dark-on-dark and invisible.
        style { dangerous_inner_html: THEME }
        div {
            style: "max-width: 46rem; margin: 2rem auto; padding: 0 1rem; \
                    color: #1a1c20; background: #ffffff; \
                    font-family: system-ui, sans-serif;",
            editor_view::Editor {
                state,
                vim: Some(vim),
                keymap: keymap.clone(),
                decorations: DecorationSource::ptr(combined_decorations),
            }
        }
    }
}
