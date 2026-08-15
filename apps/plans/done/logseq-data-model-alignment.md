# Aligning our block model with Logseq's

Reference: `~/Development/research/logseq/deps/db/src/logseq/db/frontend/schema.cljs` and the
[Vim Shortcuts plugin](https://github.com/vipzhicheng/logseq-plugin-vim-shortcuts).

The user's insight: **Logseq doesn't store pure markdown — it stores structured
blocks with first-class properties and references; markdown is the export format.**
That's why the vim plugin can wire `dd` to "delete the block entity",
`yy` to "yank the block tree", `*` to "find references to this entity",
etc. — every operation acts on a typed unit, not a text range.

Our current schema is *partially* aligned. This doc captures what we have,
what's missing, and how/when to close the gap. **Nothing in this doc blocks
the current view-mode + vim work** — both can ship against today's schema
and benefit incrementally as we close gaps.

## Side-by-side

| Logseq attr            | Our field                          | Status                    |
|------------------------|------------------------------------|---------------------------|
| `:block/uuid`          | `Block.id: Uuid`                   | ✓                         |
| `:block/parent` (ref)  | `Block.parent_block_id: Option<Uuid>` | ✓                      |
| `:block/order`         | `Block.sort_key: String` (LexoRank)| ✓                         |
| `:block/collapsed?`    | `Block.collapsed: bool`            | ✓                         |
| `:block/page` (ref)    | `Block.page_id: Uuid`              | ✓                         |
| `:block/refs` (multi-ref) | `Block.refs_json: String` (blob) | **partial** — we have the data but as a JSON blob, not indexed entity edges |
| `:block/tags` (multi-ref) | inside `refs_json` as `TagRef`   | **partial** — same        |
| `:block/properties` (entity props) | `Block.properties_json: String` (blob) | **partial** — Logseq-style `key:: value` parsed but stored as JSON, not first-class |
| `:block/title`         | `Block.content: String`            | ✓ (we keep raw markdown text) |
| `:block/created-at`    | `Block.created_at`                 | ✓                         |
| `:block/updated-at`    | `Block.updated_at`                 | ✓                         |
| `:block/journal-day`   | `Page.journal_day`                 | ✓ (page-level, not block) |
| `:block/closed-value-property` | —                          | not modeled               |

## What "first-class refs/props" buys us

1. **Backlinks panel** — "show every block that references `[[Foo]]`" is a
   constant-time query, not an O(N) scan over `refs_json` blobs.
2. **Vim `*` (search-references)** — needs a fast "what references this
   entity" lookup.
3. **Property queries** — "all blocks where `priority:: high`" works
   without re-parsing every block's content.
4. **Schema-aware editing** — the editor knows the property type (date,
   enum, ref) and can render the right widget.

## What we'd actually change

**Tier 1 — Materialized indices, no schema break** (recommended next):

- Add a `block_refs` index entity in CRDT: `(source_block_id, target_kind, target_id)`.
  Updated automatically via a Loro subscription on `Block` writes that re-runs
  `extract_refs` and diffs.
- Add a `block_props` index entity: `(block_id, key, value)`. Same write-side
  trigger, reads `properties_json` and projects into row form.
- Both indices are **derived state** — `refs_json` / `properties_json` stay
  the source of truth. The indices are caches that survive across sessions.

**Tier 2 — Property entities** (later):

- Promote `properties_json` keys to actual property entities with declared
  schema, so we can validate types, render widgets, and query.
- `Block.properties_json` becomes a write-through serialization of the
  property edges, not the source of truth.

**Tier 3 — Ref entities** (later):

- Same for `refs_json` — refs become first-class edges between block ↔ page
  / block ↔ block. The parser still extracts them from `Block.content` at
  edit time.

## How this lands alongside view modes + vim

**View / Edit / Source modes** don't depend on this work:
- Edit / View modes already render the block tree from our existing `Block`s.
- Source mode uses the existing `obsidian::serialize_page(page, blocks)` —
  works whether refs are blobs or edges.

**Vim mode v1** (motions, inserts, block ops) doesn't depend on this work:
- `dd`, `o`, `O`, `>>`, `<<`, `j`, `k`, `gg`, `G` all act on `Block` directly.

**Vim mode v2+** (`*`, marks, jumps to refs, property toggles) starts to
benefit from Tier 1 indices. That's when the work earns its keep.

## Decision

- **Now:** continue view-modes + vim coverage against current schema.
- **Soon (next plan turn):** Tier 1 — add `block_refs` and `block_props`
  indices wired via Loro subscriptions. Roughly two days.
- **Later:** Tier 2 / Tier 3 once we hit a feature that demands them.

## Progress log

- ✅ **Tier 1 shipped** — `BlockRefEdge`, `BlockPropEdge`,
  `PagePropEdge` entity_crdt repos exist + are populated by
  `reindex.rs` on block writes. Backlinks panel, `{{query}}`
  evaluator, and tasks-kanban consume them.
- ✅ **Vim binding additions** (from coverage map): `cc`
  (change block), `J` (join next), `x` (delete char — properly
  wired now via cursor offset).
- ✅ **Find-char operators**: `f{ch}` / `F{ch}` / `t{ch}` /
  `T{ch}` plus `;` / `,` (repeat / reverse-repeat). New
  `Motion::FindChar { ch, direction, till }` variant; engine
  intercepts via `PendingOp` state before the keybindings
  graph so the next keystroke becomes a literal char parameter.
  `apply_motion` impl bounds the search to the current line
  and snaps to UTF-8 boundaries.
- ✅ **Marks**: `m{a-z}` / `'{a-z}` via `SetMark(char)` /
  `JumpToMark(char)` actions. Engine handles the
  pending-next-char; host owns the `HashMap<char, Cursor>` in
  `vim_marks` and applies jumps by setting `cursor_state` +
  `editing_id` to the stored position. 37 vim tests pass.
- Pending from coverage map: `/`/`n`/`N` (search — needs UI
  input), command mode `:`, visual line `V`.
- Pending Tier 2: property entities with declared schema (only
  worth it when we ship a property-aware editor widget).
- ✅ **Inline-markdown additions** (rendered in BlockView /
  BlockNormalView via `InlineNode`):
  - `~~strike~~`, `==highlight==`, `[label](url)` external links
    (target=_blank, rel=noopener), `![alt](url)` inline images.
  - Parser handles GFM strike, Obsidian highlight, distinguishes
    `[[wikilink]]` from `[md-link](url)` and `![[embed]]` from
    `![img](url)` via run-order in `parse_inline`.
  - 6 new tests in `inline_md::tests`.
- ✅ **GFM tables**: block-level. `parse_table` detects
  `| h1 | h2 |\n|---|---|\n| a | b |` grammar (`:--`, `:--:`,
  `--:` for alignment). Renders as real `<table>` with thead/
  tbody, per-column `text-{align}` classes, alternating row
  hover, overflow-x-auto wrapper. Header + cell content
  re-runs through `parse_inline`.
- ✅ **Code block polish**: `block.kind == "code"` blocks now
  render with a header chip (language label from `code_lang` or
  "plain") + hover-visible Copy button. Body is `<pre><code>`
  with whitespace-pre-wrap, monospace, soft border. Copy uses
  the browser clipboard API via `document::eval`.
- ✅ **Footnotes**:
  - Inline `[^id]` → `Inline::FootnoteRef`. Renders as a
    superscript link pointing to `#fn-<id>`. Parser is careful
    to leave `[^id]:` at start-of-string for the block-level
    definition handler.
  - Block-level `[^id]: body` → renders with `id="fn-<id>"`
    anchor so inline refs jump to it. Body re-parses through
    `parse_inline`.
- ✅ **Obsidian callouts**: `> [!type] Title\n> body` blocks
  render as colored cards with kind-aware icons (Info / Lightbulb
  / TriangleAlert / CircleAlert / CircleCheck / Quote). Body
  lines re-run through `parse_inline` so emphasis + links nest.
  Types: note, info, tip, success, warning, danger, failure, bug,
  question, quote, example (+ synonyms). 3 callout-parse tests.

## Vim binding coverage map

The plugin ships 63 bindings. Our v1 covers ~12. The rest, grouped, in
priority order — each can land as its own file under
`crates/vim/src/bindings/`:

**Motions (most useful first):**
`w`, `W`, `b`, `B`, `e`, `E` (word motion); `f`/`F`/`t`/`T` (find char);
`/`, `n`, `N` (search); `%` (matching pair); `gj`/`gk` (visual-line).

**Block ops:**
`yy`, `p`, `P` (yank/paste block); `cc` (change block); `J` (join next
line); `>` / `<` (indent in visual mode);
`m{a-z}`, `'{a-z}` (marks).

**Counts + registers** — engine work, not per-command. Affects every
motion and operator.

**Command mode (`:`)** — palette with `:help`, `:w`, `:q`, page
navigation, etc. New mode + a `Command` variant in `VimMode`.

**Visual line (`V`)** — extend selection by whole blocks. New mode
variant.

Each command file mirrors the plugin layout — small, focused,
parameterized by the engine. Group files by category:

- `bindings/motions.rs`
- `bindings/inserts.rs`
- `bindings/blocks.rs`
- `bindings/yanks.rs`
- `bindings/marks.rs`
- `bindings/search.rs`
- `bindings/command_mode.rs`
