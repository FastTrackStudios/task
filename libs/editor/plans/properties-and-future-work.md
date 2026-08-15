# Properties + post-MVP follow-ups

State of play after the editable-frontmatter slice (this branch):
properties render type-aware, edit inline, write back via
`apply_property_change` → `Changes::replace(prop.range, …)`.
What's left is the polish + the things the MVP deliberately
deferred. Listed in rough do-this-first order.

## Done in this branch (post-plan)

- ✅ **Vim coverage**: `C`, `D`, `Y`, `gg`, `gu`/`gU`/`g~` + their
  doubled forms (`guu`/`gUU`/`g~~`). 7 new tests in
  `editor-vim/tests/vim.rs`.
- ✅ **Rename**: `data-math-pos` → `data-focus-pos`; JS message
  `math-focus` → `focus-pos`. One generic widget-click path.
- ✅ **Dead code**: removed `apply_property_edit` (unused),
  cleaned an unused import, `#[allow(dead_code)]` on
  `ensure_mark_chain` (kept for future use).
- ✅ **UX**: clicking anywhere on a property row now focuses
  the value cell (carets to end of contenteditable content).
- ✅ **Properties vim row-nav**: widget is always shown when
  frontmatter exists (no source/widget toggle on caret in).
  In Normal mode with caret inside the FM range, `j`/`k`
  hop between rows, `i`/`a`/Enter flips vim to Insert and
  focuses the row's value cell. `Esc`/`Enter` in the cell
  blurs, sends `prop-leave` → Rust flips vim back to Normal
  and refocuses the editor. Active row highlighted via
  `.is-vim-active` (amber); cell-focus row uses
  `.is-active` (blue).
- ✅ **JS capture-phase guard**: keystrokes inside a property
  cell `stopPropagation` so vim/keymap don't double-fire.
- ✅ **Playwright matrix** (4 new specs in `tests/editor.spec.js`):
  frontmatter widget renders; bool toggle writes back; text
  edit + Esc commits; list-chip add via Enter appends to YAML
  block list.
- ✅ **File splits**:
  - `markdown.rs` → `markdown/frontmatter.rs` (parser,
    serializer, render) + `markdown/typst.rs` (kinds,
    `render_typst`, LRU cache).
  - `editor.rs` → sibling `bridge.rs` for `handle_bridge_msg`
    + property edit helpers. `push_selection` / `diff_text`
    promoted to `pub(crate)`.
  - markdown.rs: 2452 → 1967 LOC. editor.rs: 2207 → 1839 LOC.
- ✅ **Typst compile debounce**: per-pass budget caps cold
  compiles at 2 per `live_preview`. LRU cap raised 32 → 128.
  Pasting a doc full of fresh equations no longer freezes —
  math fills in over a few render cycles.
- ✅ **Add / remove property rows**: each row has a hover-shown
  × handle (`prop-remove` → `Changes::delete(prop.range)`); a
  "+ Add property" cell at the bottom of the panel commits a
  new empty key on Enter (`prop-add` → insert at
  `fm.closer.start`).
- ✅ **Multiline YAML scalars**: parser handles `key: |\n  …`
  block scalars (auto-detected indent, blank lines preserved
  inside the block, trailing blanks stripped). Serializer
  emits `|` when a text value contains `\n`. Round-trip tested.
- ✅ **Vim search**: `*` / `#` search for the word under
  caret (whole-word match, wraps at doc bounds); `n` / `N`
  repeat / reverse. Last search lives in `VimState.last_search`.
  5 new tests.
- ✅ **OFM feature audit + gap fills**: inline footnotes
  `^[…]`, block IDs `^block-id`, autolinks `<url>`,
  setext-style headings (`===`/`---` underline → H1/H2 with
  HR disambiguation), and Tasks-plugin-style custom
  checkboxes (`- [/]`, `- [>]`, etc.). 7 new markdown tests.
- ✅ **Edit commands**: `Mod-k` (toggle link), `Mod-l`
  (list cycle: none → `-  → 1. → - [ ]  → none`), `Mod-t`
  (task toggle, promotes paragraphs), `Mod-1..6` (set
  heading), `Mod-0` (strip heading). All operate on every
  line in the selection. 8 new tests.
- ✅ **Slash-command palette**:
  - `slash` module in editor-view: `SlashState`,
    `detect_slash`, `CommandEntry`, `CommandKind`,
    `filter_commands`, `run_command`. Ported from
    `~/Development/Task/.../handler/commands.rs`. 6 unit tests.
  - Catalog: headings 1-6, lists (unordered / ordered /
    task / quote / hr / table), code fences (generic / rust
    / ts / typst / mermaid), math (inline / block), all 13
    Obsidian callouts, links (link / wikilink / embed /
    footnote / inline footnote).
  - `SlashMenu` Dioxus component renders the popup; reads
    open state from a `Signal<Option<SlashState>>` threaded
    through Editor's new `slash` prop.
  - Editor's `use_effect` re-runs `detect_slash` on every
    doc/selection update; `onkeydown` intercepts
    Arrow/Enter/Escape when the menu is open.
  - Architecture parallels CodeMirror's
    `@codemirror/autocomplete`: `detect_slash` is the
    `matchBefore` analogue, `SlashState` is `ActiveSource`,
    `CommandKind` is `Completion.apply`.

Still pending below.

### Next concrete chunks

- **Mermaid fence rendering**. Add `editor-mermaid` crate
  wrapping `mermaid-rs-renderer = "0.2.2"` with
  `default-features = false`. Cargo dep is wasm-compatible per
  scout (pure Rust, no rayon/stacker; bundle a WOFF/TTF via
  `include_bytes!` since `fontdb` system-load won't work in
  the browser). Mirror `editor-typst`'s `Compiler::compile_svg`
  shape. Hook into the existing ` ```typst ``` `-fence handler
  pattern in `markdown.rs::scan_blocks`.
- **Caret-anchored slash-menu position**. Today the popup
  docks at the bottom of the editor frame. A sliver of JS
  reading `window.getSelection().getRangeAt(0).getBoundingClientRect()`
  would let it float just under the caret like Logseq.
- ~~**Combination edge cases**: code fence inside a callout
  with `>`-prefix stripping; nested callouts (`> > [!note]`).~~
  Nested callouts are done. Code-fence-in-callout is
  **dropped on purpose** — code fences are raw-code
  territory, no markdown processing inside them.  If the
  user wants a fence with explanatory prose, the fence lives
  outside the callout. Document it as "by design" so a future
  pass doesn't re-litigate.
- **Vault-aware wikilink resolution**. Today every wikilink
  marks `md-wikilink-unresolved`. When a vault index lands,
  swap to `md-wikilink-resolved` (purple) when the target
  exists. Hook: thread a `vault: Option<&dyn VaultLookup>`
  into `live_preview`.

The two files most of this work touches:
- `crates/editor-state/src/markdown.rs` — 2.4k lines, doing
  most of the heavy lifting for live-preview decorations.
- `crates/editor-view/src/editor.rs` — 2.2k lines, the
  Dioxus component + JS bridge + patcher glue.
Both are getting long. Splitting is a code-quality task below.

---

## 1. Properties: vim row-as-line interaction

Goal: caret on a property row in normal mode behaves like a
Vim line — `j`/`k` between rows, `i`/`a` opens the value
cell, `Esc` blurs and returns. Matches the Obsidian feel.

**Why this is non-trivial.** The properties panel is a
`Decoration::widget` — it isn't part of the cm-line text
flow, so vim's existing line motions don't see its rows. The
options:

- **A. Virtual row anchors.** Emit a hidden `<span>` per row
  with a known caret offset (e.g. the row's `range.start`).
  Vim's `j`/`k` then maps to those anchors. Source visibility
  toggle changes from "any caret inside the FM range" to
  "caret inside a *value cell*". Lowest invasive change to
  the vim engine; relies on the patcher reading anchor
  positions through `data-tile-len`.
- **B. Widget-aware vim layer.** Add a `Mode::Widget` (or a
  `widget_focus: Option<WidgetId>` on `VimState`) so vim
  re-routes motion/insert when a widget claims focus. More
  general, but invents a new state machine.

**Recommend A.** Smallest change to the vim engine, fits the
existing motion → caret-position model. `i`/`a` from a row
anchor dispatches `math-focus`-style message that focuses
the cell instead of moving caret.

Extra: a one-row visual highlight (no actual `VisualLine` —
just a CSS class on the active row) when the caret sits at
a row anchor.

**Touch points:**
- `crates/editor-state/src/markdown.rs::render_properties_html`
  — embed `<span data-row-anchor="N"></span>` per row.
- `crates/editor-vim/src/state.rs` — `i`/`a` checks the
  current caret against row anchors; on hit, dispatch the
  focus message rather than entering text insert mode.
- `crates/editor-view/src/editor.rs` — new `prop-focus`
  bridge message → focus a specific row's value cell.

---

## 2. Vim coverage gaps

The vim engine in `crates/editor-vim/src/state.rs` covers
motions and the common operators, but several Obsidian /
neovim-default bindings aren't wired. Each is small in
isolation; bundle them as one "vim coverage" PR.

- `C` — change to EOL (`c$`). Implement as `D` + enter insert.
- `D` — delete to EOL. Mirror of `S`'s line-end motion.
- `Y` — yank line (`yy`). Same body as `dd`, no delete.
- `<` / `>` — indent / outdent. Linewise on `<<`/`>>`,
  range-wise after an operator. Needs to know the indent
  unit (2-space default, settings-driven later).
- `gu` / `gU` / `g~` — lowercase / uppercase / toggle-case
  over a motion. Toggle-case-char (`~`) already exists.
- `J` — join lines. Strip the newline + collapse leading
  whitespace.
- `*` / `#` — search word under caret. Depends on a search
  primitive; defer to the search-bar slice.
- `q`/`@` — macro record / replay. Stubbed in
  `editor-vim/src/macros.rs` (note the `LastChange::Insert(_) =>
  None, // v1: TODO`). Closing this gap means recording the
  full key stream, not just the last edit.

Repeatable test fixture: extend `crates/editor-vim/tests/`
with one snapshot per binding (input doc, key string, output
doc + selection). Use the same builder we use for motions
today.

---

## 3. Properties polish

- **YAML library swap.** Hand-rolled parser handles the
  Obsidian shape but doesn't deal with nested maps,
  multi-line strings (`|`/`>`), or anchors. Move to
  `saphyr` (lighter than `serde_yaml`, still on wasm). Keep
  the byte-range-per-property model — that's what makes the
  current edit path cheap.
- **Add / remove properties.** A `+ Add property` row at the
  bottom; an `x` on hover at the row level. Adds need a key
  picker (later: type registry); removes are a
  `Changes::delete(prop.range)`.
- **Type registry.** Obsidian has a per-vault map of `key →
  type`. Without it we can't disambiguate `1.0` ("text") from
  `1.0` (number) for keys the user has explicitly typed.
  Defer until we have a settings file format.
- **List add UX.** Today the chip-add input commits only on
  Enter; comma should also commit, and Backspace on empty
  input should focus the last chip's remove button.
- **Empty / null distinction.** Right now `PropValue::Empty`
  always writes `key:`; YAML also has `key: null` and `key:
  ~`. Not worth caring about until users complain.
- **Multiline string values.** Block scalar (`|`) for long
  notes / descriptions. Today they collapse to a single line.

---

## 4. Typst + math rendering

- **Compile budget.** `render_typst` runs synchronously on
  every live-preview pass. A doc with 50 inline equations
  pays 50× cache lookups per keystroke. The cache hits keep
  this cheap once warm, but the first render of a doc full
  of math is visibly laggy. Two paths:
  - Debounce compiles via a per-source `compile-after` token;
    show the source while the compile is in flight, swap to
    the widget when it lands.
  - Larger LRU cap (today: 32) + persist across docs in a
    web-worker. The worker path also opens up parallelism.
- **Bigger world.** Today `editor-typst::World` has no file
  resolver — imports / packages won't work. Adding the
  `@preview` registry needs a file-cache + network fetch;
  defer until users hit it.
- **PDF export.** Wired in `editor-typst` but not surfaced.
  When we add it, route through the existing command palette;
  the result is a `Vec<u8>` that JS turns into a blob URL.
- **`$x$` strictness.** Our inline scanner accepts `$5 $10`
  patterns when there's no whitespace after the closing `$`.
  Tighten: require a non-`$` body and trailing punctuation
  / whitespace.

---

## 5. Performance

The two hot paths are `live_preview` (per-keystroke) and the
DOM patcher (per-decoration-set diff).

- **Profile first.** `now_ms_native()` instrumentation in
  `live_preview` already prints block/inline phase timings
  to the console on wasm. Add a histogram (last 60 frames)
  and a `?perf=1` query flag to render it as an overlay.
- **Avoid full re-scan on selection-only changes.** Today
  every transaction triggers `live_preview`. When the
  transaction only changes selection (no `Changes`), only
  the `cursor_touches`-gated decorations need to be
  recomputed; the block scan can be cached against the doc
  version.
- **Tree-sitter fence cache.** Already cached by source
  string (in `FenceCache`); the cache key is per-language but
  not per-doc, so two docs with identical Rust snippets
  share. Verify the cap is sane (currently…?) and add a
  `clear()` hook on language pack swap.
- **Patcher diff.** `tile/build.rs` walks the full event
  stream every frame. Heuristic: skip the prefix where
  events haven't moved and the doc bytes are unchanged. CM6
  does this via "viewport changesets"; mirror that.
- **Selection messages.** `sel` events from the browser fire
  on every cursor blink in some cases — debounce on the JS
  side with a `requestIdleCallback` so we don't flood the
  bridge.

---

## 6. Code quality

- **Split `markdown.rs`.** 2.4k lines split naturally:
  - `markdown/scan.rs` — block + inline scanners
  - `markdown/frontmatter.rs` — parser, serializer, render
  - `markdown/widgets.rs` — embed / table / typst / math
    widget HTML builders
  - keep `markdown/mod.rs` as the public `live_preview`
    facade.
- **Split `editor.rs`.** 2.2k lines, similar shape:
  - `editor/bridge.rs` — JS message handler (currently
    `handle_bridge_msg`)
  - `editor/dom.rs` — `posFromDOM`, selection writeback
  - `editor/component.rs` — the Dioxus component itself
- **`escape_html`.** Defined in `markdown.rs` but also a
  good utility for the patcher; lift to a `editor-utils`
  crate (or `editor_state::util`) and share.
- **Dead-code sweep.** `cargo check` already emits a couple
  of unused-import warnings in `editor-view`. Clean up.
- **Naming.** `data-math-pos` is now used by typst widgets,
  property panels, and inline math — rename to
  `data-focus-pos` (or `data-source-pos`) and update the JS
  handler.
- **`apply_property_edit` is unused.** Wrote it as a public
  helper, then ended up using `parse_frontmatter` +
  `serialize_property` directly. Either remove or use it.

---

## 7. Testing

- **Playwright matrix.** `tests/playwright.config.js` runs on
  port 9091. Cover:
  - Frontmatter render + click each cell type → expect
    correct input focus.
  - Each list chip add / remove → doc text matches expected
    YAML.
  - Inline math compile + click to edit.
  - Task toggle preserves caret (regression for the
    `mousedown` preventDefault fix).
  - Vim row-nav (after slice 1 lands).
- **Snapshot rendering.** Add `insta` for the patcher
  output. Today we test decorations; we don't assert what
  the final DOM looks like. A handful of snapshots
  (heading, callout, table, code fence, properties, math)
  catches regressions cheaply.
- **Property fuzz.** `cargo fuzz` over `parse_frontmatter` →
  `serialize_property` → `parse_frontmatter`. Round-trip
  must equal the original prop list for any input the parser
  accepts.
- **Wasm playground build.** The pre-existing
  `openssl-sys` / `tokio` errors on `cargo check
  --target wasm32-unknown-unknown` (whole workspace) need a
  diagnosis — likely a dev-dep escaping the `[target]`
  gate. Editor crates themselves are clean.

---

## Suggested order

1. Vim coverage (`C`, `D`, `Y`, `<`/`>`, `J`) — small,
   isolated, builds test muscle.
2. Code quality split (markdown / editor) — easier to do
   before the file gets larger.
3. Properties vim row-nav (slice 1 of this doc).
4. YAML lib swap + add/remove rows.
5. Performance: debounce typst + selection-only path.
6. Playwright matrix + snapshot tests.

Macros (`q`/`@`), search (`*`/`#`), and full type registry
are each their own slice and shouldn't gate the above.
