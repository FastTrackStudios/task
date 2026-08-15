# Block IDs + references + embeds (Logseq-style)

Hybrid editor that does both Obsidian-flow editing AND
Logseq-style block references. Refs must resolve from the
markdown files alone — no separate index, no server.

## Decisions locked

**Identity**: every block gets a UUID v4 (DataScript-style
squuid so they're time-ordered for stable sort). The UUID is
the source of truth.

**On-disk syntax** (Logseq-compatible):
```markdown
This is a paragraph with content.
id:: 5f9c1234-abcd-...

((5f9c1234-abcd-...))      — block reference
{{embed ((5f9c1234-...))}} — block embed
```

The `id:: <uuid>` line is **always written on the line
immediately after the block** when the block has an id. No
trailing comment, no inline form.

**When ids are written**: lazily. A block gets a UUID
internally (in our parsed model) when the user runs the
"create block id" command. The `id::` line lands in the file
on the next save. Unreferenced blocks stay clean — files only
get UUIDs where you actually need them.

**Rendering** (live-preview AND reading mode):
- `id::` lines are **completely hidden** via `Decoration::replace`
  over the whole line. Not even visible when the caret is on
  the line — we don't want users editing or deleting them by
  accident.
- A small **🔗 indicator** appears at the END of the block
  that has an id, signalling "this block is referenceable".
  Click the indicator → copies `((uuid))` to clipboard.
- `((uuid))` is rendered as an **atomic chip** showing the
  first ~40 chars of the target block's content. Click the
  chip → navigate to the target. Source UUID is hidden;
  arrow keys / `Backspace` treat the chip as one unit.
- `{{embed ((uuid))}}` renders the **target block's full
  content inline** in a bordered "embed card". Click → jump
  to the source.

**Raw source mode**: a future toggle (`Mod-/`?) shows the
underlying markdown including `id::` lines and raw `((uuid))`
strings. v1 doesn't include the toggle — files always render
in live-preview shape. Users can `cat` the `.md` if they need
the source.

**Resolution**: refs resolve by grepping the vault for the
matching `id:: <uuid>` line. Single-file vault: just the
current doc. Multi-file: a synchronous full-vault scan
(cached in memory per session). No persistence needed —
rebuildable from the files.

## Why these shapes

- UUID > short slug because refs must survive moving a block
  to a different page without any rewrite step. Logseq's
  `((uuid))` form is page-free; resolves anywhere in the
  vault.
- Hide the UUID entirely (not just when caret-off) because
  editing it would break every reference to that block.
  Showing it invites the user to break it.
- Lazy disk-write keeps files clean — most blocks never get
  referenced, no reason to dirty the file.
- 🔗 indicator at end of the block (not start) because the
  block's title/first line is the important thing; the id is
  a footnote.

## Done — multi-file resolution

- ✅ `VaultLookup` trait in `editor_state::markdown`: methods
  for `lookup_block(uuid)`, `lookup_page(name)`,
  `lookup_section(page, heading)`,
  `lookup_block_short(page, short_id)`. `editor-state` stays
  vault-agnostic; the `vault` crate provides the canonical
  impl (`VaultLookupView::new(&vault, &block_index)`).
- ✅ `live_preview_with(state, Some(&lookup))` —
  vault-threaded entry point. The old `live_preview(state)`
  still works (passes `None`); existing callers don't change.
- ✅ `((uuid))` block-ref resolves in three steps: intra-doc
  index → vault → unresolved. Resolved refs show
  `🔗 preview › page-name` so the user knows where the target
  lives.
- ✅ `{{embed ((uuid))}}` block-embed card shows the target
  page as a header chip + content body.
- ✅ `![[Page]]`, `![[Page#Heading]]`, `![[Page#^short-id]]`
  cross-doc resolution via the vault. Intra-doc forms
  (`![[#…]]`) still hit the local scanner first.
- ✅ `[[Page]]` wikilink resolved/unresolved class flips
  based on vault.lookup_page existence — drops the "always
  red" placeholder behavior.
- 3 new editor-state tests + 4 new vault tests cover the
  cross-doc paths end-to-end.

## Done — block-mode core

- ✅ UUID v7 generation (time-prefixed, sortable). Switched
  from v4 — Logseq's squuid model. Version-tracking friendly.
- ✅ `((uuid))` block-ref chip + `{{embed ((uuid))}}` block-
  embed card.
- ✅ `id:: <uuid>` hidden + atomic in live-preview.
- ✅ `Mod-Shift-K` / `/block-id` command.
- ✅ Page embed `![[Page]]`, section embed `![[Page#Heading]]`
  / `![[#Heading]]`, block embed via Obsidian short-id
  `![[Page#^id]]` / `![[#^id]]`. Intra-doc resolution lands
  for the `![[#…]]` forms (page-empty); cross-doc renders a
  "multi-file lookup pending" placeholder until the vault
  index slice.

## Implementation slices

### 1. Parser (this PR)
- New inline span: `((<uuid>))` → `md-block-ref` class +
  `data-uuid` attr.
- New inline span: `{{embed ((<uuid>))}}` → `md-block-embed`
  class.
- New block-level recognizer: a line that's just `id::
  <uuid>` (with optional leading whitespace) is the id of
  the **previous block**. Emit `Decoration::replace` over the
  whole line. Stash the UUID + previous-block-line offset in
  a side channel for the live-preview to use.

### 2. Decorations (this PR)
- `((uuid))` → atomic chip widget showing target preview.
  Click navigates.
- `{{embed ((uuid))}}` → bordered card with the target's
  rendered content.
- 🔗 indicator widget at the end of any block that has an
  id. Click copies `((uuid))` to clipboard.

### 3. Vault index (this PR)
- A thread-local `BlockIndex` mapping `uuid → block content`
  rebuilt at the start of every `live_preview` pass over the
  current doc. Multi-file scan is a later slice.

### 4. Commands (this PR)
- `Mod-Shift-K` and `/block-id` — generate a UUID for the
  block at the caret and append `id:: <uuid>` line below.
  Copy `((uuid))` to clipboard.

### 5. Multi-file resolution (later)
- Walk all `.md` files in the vault, build a global `uuid →
  (file, content)` map. Invalidate on file change.

### 6. Outline mode (later — separate plan)
- Per-doc frontmatter `outline: true` switches the editor
  into Logseq-bullet rendering. Every bullet is a block;
  Enter creates a new bullet at the same indent; Tab/Shift-
  Tab adjusts indent. Out of scope for the block-id PR.

## Tests

- Parser: `id:: <uuid>` recognized, `((uuid))` inline ref
  matched, `{{embed ((uuid))}}` matched, garbage UUIDs
  ignored.
- Decorations: `id::` line replaced, `((uuid))` rendered as
  chip with target preview, 🔗 indicator appears on blocks
  with ids.
- Commands: `Mod-Shift-K` adds an `id::` line + copies the
  ref string to clipboard.
- Resolution: `((uuid))` whose target exists shows target
  preview; missing target renders as a red "unresolved" chip.
