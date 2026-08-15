# Logseq-style outliner on /knowledge

Replace the current flat `BlockRow` textarea list with a true
recursive outliner. Match Logseq's affordances and keyboard model,
render with architect-ui tokens (not a pixel clone).

Reference checkout: `~/Development/research/logseq` —
`src/main/frontend/components/{block,editor,page}.cljs`. Don't port
Clojure; port behavior.

## Data model — already in place

`features/knowledge/knowledge-proto/src/lib.rs` (lines 219–302) has:

- `parent_block_id: Option<Uuid>` — tree parent
- `sort_key: String` — LexoRank fractional index among siblings
- `collapsed: bool` — fold state, persisted
- `kind`, `heading_level`, `list_ordered`, `list_task`, `properties_json`, `refs_json`

CRDT repo (`BlockRepoLoro`) already supports create/update/delete on
these fields. **No schema changes required for v1.**

## Phases

### Phase A — Tree rendering + indent/outdent + fold

1. New module `features/knowledge/knowledge-ui/src/outliner.rs`.
2. `OutlinerTree` component: takes `Vec<Block>` for the page, builds
   `parent_block_id → children` map sorted by `sort_key`, renders a
   recursive `OutlinerNode`.
3. `OutlinerNode`: bullet (•) + fold chevron (▸/▾) + content area +
   children. Indent via left padding `depth * 1.25rem`. Indent guide
   = thin left border on the children container.
4. Keyboard model on the content textarea:
   - **Tab**: indent (new parent = previous sibling).
   - **Shift-Tab**: outdent (new parent = grandparent; place after
     current parent).
   - **Enter**: split block at caret. Create new sibling after this
     one with the post-caret text; truncate current to pre-caret.
     Empty + leaf + non-root: outdent instead.
   - **Shift-Enter**: insert literal newline.
   - **Backspace at offset 0**: merge into previous sibling (or
     outdent if first child).
   - **Cmd/Ctrl-↑/↓**: move block up/down among siblings (lexorank
     reorder).
   - **Cmd/Ctrl-Shift-↑/↓**: collapse / expand.
5. Click chevron toggles `collapsed` on the block.
6. Drop the autoresize textarea hack — use a `contenteditable` div
   so we can do caret-offset queries (needed for Enter-split and
   slash/[[/]] triggers in Phase B/C).

### Phase B — `[[Page links]]` autocomplete

1. Detect `[[` typed in the contenteditable. Track the query string
   between `[[` and the caret.
2. Floating panel anchored to the caret (use existing architect-ui
   popover/listbox if available, else handroll). List pages from
   the current snapshot, fuzzy-matched.
3. Enter / click → replace `[[query|` with `[[Page Name]]` and move
   caret past `]]`. Esc closes the panel.
4. Already-rendered `[[Page Name]]` segments inside saved content
   render as clickable links — clicking sets `selected_page` to
   that page's id. (Same behavior on `#tags`.)
5. **Out of scope for v1**: page renames cascading. The link is
   stored as text; renames will surface a "broken link" indicator
   later.

### Phase C — Slash commands

1. Detect `/` at start of block or after whitespace. Open command
   palette below the caret.
2. Static command list for v1:
   - `Heading 1/2/3` → set `kind=heading`, `heading_level=1..3`.
   - `Bulleted list` / `Numbered list` → `kind=list_item`,
     `list_ordered=false/true`.
   - `Todo` → `kind=list_item`, `list_task=Some(" ")`.
   - `Code block` → `kind=code`.
   - `Quote` → `kind=blockquote`.
   - `Today's date` / `Tomorrow` → insert ISO date as text.
3. Enter applies the command, removes the `/query` from content,
   closes the palette.

## Out of scope (v1)

- Block refs `((uuid))` — needs cross-page resolution & sidebar.
- Right sidebar / stacked panes.
- Cmd-K command palette / quick-switcher.
- Journals route.
- Graph view.
- Drag-and-drop reorder of blocks (keyboard reorder via
  Cmd-↑/↓ covers the keyboard path; HTML5 DnD can come after).
- Inline `prop:: value` chips — `properties_json` is already
  parsed at repo level; rendering as chips is a follow-up.

## Test plan

- `cargo check -p knowledge-ui` + wasm clean.
- Playwright spec `tests/playwright/outliner.spec.js`:
  - Type two lines separated by Enter → two sibling blocks.
  - Tab on second → becomes child of first.
  - Shift-Tab → back to sibling.
  - `[[` opens autocomplete; Enter selects first match.
  - `/heading` + Enter sets H1 styling.
- Existing knowledge-route specs must keep passing (delete-on-
  empty-Backspace, sidebar search, page rename).
