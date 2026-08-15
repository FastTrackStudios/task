# Plan: LoroText Upgrade for Block-Level Text Editing

**Status**: Open. Discovered while debugging editor duplication during the knowledge feature's v1.5a live-preview rollout (commit `b527715`). The immediate prefix-out-of-contenteditable bug is fixed; this is the deeper architectural follow-up.

**Scope**: Replace `Block.content: String` (plain Loro map field, LWW per write) with a Loro `LoroText` container that merges character-level edits across peers. Update every editor input path to emit text *diffs* rather than full-content writes.

**Why now**: Two browser tabs editing the same block currently lose data — every keystroke writes the entire new content via map LWW. The whole point of the local-first, realtime, collaborative architecture is that two peers editing simultaneously should both end up with a merged result. LoroText delivers that natively.

---

## Background — how it works today

`Block.content` is plumbed as a plain Rust `String` end to end:

1. `knowledge_proto::Block.content: String` — wire type.
2. `knowledge-crdt::EntityCrdt for BlockEntity::encode_into` calls `write_str(m, "content", &e.content)` — sets the `content` key on the Block's LoroMap to a string LoroValue.
3. The editor (`knowledge-ui::editor::block.rs`) reads the textContent of the contenteditable div on every `oninput`, calls `on_content_change(full_string)`, the route handler builds a `BlockUpdate { content: Some(full_string), ..Default::default() }` and writes through the repo.
4. Every keystroke = one full-content write. Concurrent edits from two peers = one wins, the other's keystrokes vanish.

This is fine for low-churn fields (`Page.basename`, `Vault.name`) but pathological for the actual editing surface.

## Background — what Loro provides

[Loro's docs](https://loro.dev/docs/tutorial/text) describe `LoroText` as the CRDT text container. API roughly:

```rust
let text: LoroText = doc.get_text("content");
text.insert(pos, "hello")?;       // insert at UTF-16 code-unit pos
text.delete(pos, len)?;           // delete N units starting at pos
let s: String = text.to_string(); // full read
text.subscribe(|event| { … });    // diff events on remote merges
```

Two peers concurrently inserting at the same position both keep their characters; deletes and inserts merge per the CRDT semantics. We get character-level merge without writing any merge logic.

## What needs to change

### 1. `crdt` lib — new codec helpers

The shared `crdt` crate (at `../architect/libs/crdt`) only exposes `read_str` / `write_str` for string fields, both of which thunk through `LoroMap::insert_string`. We need new helpers that anchor a `LoroText` child container under a key.

Add to `crdt::codec`:

```rust
/// Get-or-create a LoroText child under `map[key]`. Returns the
/// container so callers can `to_string()`, `insert(...)`, `delete(...)`.
pub fn text_child(m: &LoroMap, key: &str) -> Result<LoroText, RepoError>;

/// Read the current text value. Equivalent to `text_child(m, key).to_string()`.
pub fn read_text(m: &LoroMap, key: &str) -> Result<String, RepoError>;

/// Apply a sequence of edit ops (insert / delete) against the text
/// container. Each op is in UTF-16 code-unit coordinates (Loro's
/// native unit).
pub fn apply_text_ops(m: &LoroMap, key: &str, ops: &[TextOp]) -> Result<(), RepoError>;

pub enum TextOp {
    Insert { pos: u32, text: String },
    Delete { pos: u32, len: u32 },
}

/// Diff helper for callers that only have old + new strings (e.g.
/// fallback path for paste / programmatic content overwrite). Computes
/// a minimal insert/delete sequence and applies. v1: use a naive
/// common-prefix/suffix diff — O(n) and correct for typing flows.
/// FUTURE: swap for `similar` or `difference` for non-prefix changes.
pub fn apply_text_diff(m: &LoroMap, key: &str, old: &str, new: &str) -> Result<(), RepoError>;
```

These wrap `LoroMap::get_or_create_container::<LoroText>(...)` and the Loro text API. Add 6–8 unit tests covering: get-or-create idempotence, insert+delete sequencing, diff round-trip, multi-peer merge (two `LoroDoc`s receiving disjoint insert ops at the same position then importing each other's updates).

### 2. `knowledge-crdt::BlockEntity`

Update three impl methods:

- `encode_into`: replace `write_str(m, "content", &e.content)` with logic that initializes the text child if the block is new, then applies the full `e.content` via `apply_text_diff(m, "content", "", &e.content)`. The "encode entire entity" path only runs on create, so a single insert is fine.

- `decode_from`: replace `content: read_str(m, "content")?` with `content: read_text(m, "content")?`.

- `apply_update`: when `u.content.is_some()`, **stop writing through `BlockUpdate.content: Option<String>`** — that's the bug. Architect-emitted `BlockUpdate` is a struct of `Option<T>` fields that says "set this field to this value". For LoroText, "set" doesn't make sense — we want "apply these ops".

  Either:
  - **Option A (preferred)**: keep `BlockUpdate.content: Option<String>` for compat but compute the diff internally: read the existing text, apply `apply_text_diff(old, new)`. Loses the diff fidelity since callers had to materialize a full string first, but at least no map-level LWW.
  - **Option B (proper)**: introduce a sibling update type `BlockTextOps { ops: Vec<TextOp> }` and a new `RepoExt::apply_text_ops(id, "content", ops)` method on the repo. Callers (editor) emit ops directly. Best fidelity.

  v1 ships Option A so we don't have to retool every consumer; the editor migrates to Option B as part of the same PR.

### 3. `knowledge-proto::Block`

`Block.content: String` stays as the in-memory shape — callers always get a plain `String` back from `decode_from`. The CRDT mechanics are internal. No proto migration needed.

But: add a **new method on `BlockRepo`** (or a sibling extension trait, since `BlockRepo` is architect-generated) for the ops path:

```rust
// In knowledge-crdt:
impl BlockRepoLoro {
    pub async fn apply_text_ops(
        &self,
        block_id: Uuid,
        ops: Vec<TextOp>,
    ) -> Result<(), RepoError>;
}
```

This is the fast path the editor uses. The slow path (`repo.update(id, BlockUpdate { content: Some(...) })`) stays for cases like programmatic block creation, paste handlers that produce a complete new content, etc.

### 4. `knowledge-ui::editor::input` — emit diffs

Currently `handle_beforeinput` returns `InputCommand { new_content, new_caret, structural }`. Rework to:

```rust
pub struct InputCommand {
    pub edits: Vec<TextOp>,           // empty for structural-only commands
    pub new_caret: u32,
    pub structural: Option<StructuralOp>,
}
```

The browser's `InputEvent.inputType` tells us what kind of edit happened (`insertText`, `deleteContentBackward`, `insertFromPaste`, `insertCompositionText`, etc.) plus `event.data` for inserted text and `getTargetRanges()` for deletion ranges. Map each `inputType` to `TextOp`s:

| inputType | TextOp |
|---|---|
| `insertText` | `Insert { pos: caret, text: event.data }` |
| `insertCompositionText` | suppressed until `compositionend`; on end, single `Insert` with full composed string |
| `deleteContentBackward` | `Delete { pos: caret - 1, len: 1 }` |
| `deleteContentForward` | `Delete { pos: caret, len: 1 }` |
| `deleteWordBackward` | `Delete { pos: word_start, len: caret - word_start }` |
| `insertParagraph` | structural: `StructuralOp::SplitBlock { at: caret }` |
| `insertFromPaste` | `Insert { pos: caret, text: clipboard_text }` (after HTML strip) |
| `historyUndo` / `historyRedo` | suppress browser default; route to Loro's own undo tree |

The caret tracking from `caret.rs` (already implemented) gives us the position. Lifetimes of `event.data` and `getTargetRanges()` results are short — read them inside the handler, don't hold across awaits.

The route's `on_block_patch` callback gets a new sibling `on_block_text_ops(Uuid, Vec<TextOp>)` that calls `block_repo.apply_text_ops`.

### 5. `knowledge-ui::editor::block.rs` — wire the new pipeline

- `oninput` currently calls `ev.value()` to grab the full textContent. Switch to using `onbeforeinput` exclusively (browser fires both; we suppress the default after handling `beforeinput`, so the DOM doesn't double-apply).
- For composition (IME): listen `oncompositionstart`/`oncompositionupdate`/`oncompositionend`. Buffer the composing text locally; emit one `Insert` op on `compositionend`.
- For paste: intercept `onpaste`, prevent default, read text from `event.clipboardData()`, emit `Insert`.
- **Don't re-render the contenteditable's text from `block.content`** — let the browser keep its own DOM state. Only re-render the surrounding non-editable chrome (prefix, span decorations of non-edited parts). The CRDT echo from our own writes lands as a no-op when ops we sent match what's already in the DOM.
- Remote updates (other peers): subscribe to the LoroText at the route level; on remote diff, splice into the contenteditable via DOM mutation (preserve local caret position around the splice point using `caret.rs::pin_caret`).

This last point is the subtle one. The Obsidian / Notion / Lexical pattern is:
1. Maintain a virtual "doc state" alongside the DOM.
2. Apply local edits to both the DOM (via browser default) and the doc state (via beforeinput handler).
3. When remote edits arrive, transform them through any pending local ops, then splice into the DOM via mutation API, preserving caret.

For v1.5b we can punt the splice-on-remote logic by re-rendering the block on remote-only changes (cheaper UX but flickers caret); v1.6 lifts it.

### 6. Editor on focus blur

Currently `onblur` triggers a save by reading `ev.value()` and calling `on_content_change`. With the new pipeline that's redundant — every keystroke already wrote ops. **Remove the blur-save path entirely.** Document that the editor is now fully event-driven.

### 7. The same upgrade for `chat_proto::Message.body`

Chat messages have the same problem — once we wire it for knowledge, the chat AI streaming endpoint also benefits (server can apply text ops as the assistant streams instead of re-writing the whole body string at 1Hz). Same pattern: `Message.body` codec swaps to LoroText.

Mark this as Phase 2; ship knowledge first to validate the pattern.

### 8. Migration story

Existing snapshots store `content` as a string LoroValue, not a LoroText container. On first read of an old block:

```rust
fn read_text_with_migration(m: &LoroMap, key: &str) -> Result<String, RepoError> {
    // Try the new path first.
    if let Some(text) = try_text_child(m, key) {
        return Ok(text.to_string());
    }
    // Fall back to the old string field and migrate-on-read.
    let legacy = read_str(m, key)?;
    if !legacy.is_empty() {
        apply_text_diff(m, key, "", &legacy)?;   // seeds the LoroText
    }
    Ok(legacy)
}
```

Demo data only — no compatibility concern with shipped users — but the migration helper keeps dev databases from breaking on first run after the upgrade.

---

## Sequencing

Three commits, each independently verifiable:

### Phase 1 — `crdt` lib codec helpers
- Add `text_child`, `read_text`, `apply_text_ops`, `apply_text_diff`, `TextOp` to `crdt/src/codec.rs`.
- 8 unit tests in `crdt/src/codec/tests.rs` (round-trip insert+delete, concurrent merge across two LoroDocs, diff produces minimal ops for common-prefix changes).
- No consumer changes yet.

Verify: `cargo test -p crdt` clean.

### Phase 2 — `knowledge-crdt` migration + editor input rewrite
- `BlockEntity` swaps to LoroText for `content`; `decode_from` reads via the migration-aware helper.
- `BlockRepoLoro::apply_text_ops` method added.
- `knowledge-ui::editor::input` returns `Vec<TextOp>` instead of full strings.
- `BlockEditor` switches `oninput` → `onbeforeinput` with `prevent_default`, wires `oncomposition*`, `onpaste`.
- Route handler in `crates/task-ui/src/feature_routes/knowledge.rs` wires `on_block_text_ops` callback through to `block_repo.apply_text_ops`.
- The legacy `on_block_patch` with `content: Some(_)` path is kept for non-editor mutations (programmatic create, structural ops).

Verify:
- `cargo test -p knowledge-proto -p knowledge-crdt -p knowledge-ui` clean.
- Manual: type "hello world" into a block, see no duplication. Open a second tab, type concurrently into the same block — both peers' characters appear interleaved without loss.
- Manual: copy/paste rich text from another browser tab — body is correctly stripped to plain text and inserted as a single op.

### Phase 3 — chat message body
- Same surgery for `chat-crdt::MessageEntity`.
- Server's `apps/server/src/chat.rs` SSE streaming pipeline switches from "batched-flush every 1Hz of full body" to "emit text-insert op per token-chunk" against the assistant message's LoroText.
- Mark `Message.streaming` true while ops are flowing; false when `Done` chunk arrives.

Verify: chat-ai route streams without the 1Hz debounce; characters appear character-by-character in real time across peers.

---

## What this fixes

- ✅ Two peers editing the same block both keep their characters (no LWW data loss).
- ✅ Typing speed isn't bottlenecked by full-string serialization on every keystroke (ops are tiny: 1 char insert = ~12 bytes over the wire).
- ✅ Chat AI streaming becomes a proper character-level CRDT event stream — feels real-time.
- ✅ Future undo/redo can use Loro's own undo machinery instead of our own snapshotting.

## What this does NOT fix (and we explicitly punt)

- Tree-structured undo / branching history (Loro supports it via separate API; v1 uses linear undo only).
- Cross-block selection / shift-select-across-blocks (still a hard wall at block boundary).
- Inline live-preview span decoration during typing (currently coarse "focused→all-raw"; the proper span-level decoration is its own follow-up arc, orthogonal to the LoroText upgrade).
- IME-heavy languages with complex composition (basic compositionend handling lands; CJK polish is v1.6).

## Risk register

| Risk | Mitigation |
|---|---|
| `LoroText` API differs subtly from what we assume | Read `crdt` lib docs + Loro changelog before Phase 1; write the codec helpers against a real `LoroDoc` in tests. |
| `beforeinput` inputType coverage is incomplete | Catch unknown inputTypes as `_ => { /* fall back to oninput full-content path */ }` — degrades to current behavior, never crashes. |
| Concurrent edits at the same caret produce out-of-order characters | Loro's text CRDT handles this correctly; the only weirdness is that "abc" and "xyz" inserted at the same position from two peers interleave as e.g. "axbycz" instead of "abcxyz". Document. |
| Codec migration on read causes a write (loop / dirty state) | The migration helper writes only when `legacy != ""` AND the text container is absent. Idempotent after first read. |
| Loro op size for paste of large content | A paste of 1 MB still emits one Insert op; size is bounded by content. Loro handles this fine. |

## Reference reading before starting

- `~/Development/research/logseq/` — Logseq's outliner uses Datascript not Loro, but its block-level edit semantics are the closest analog.
- `crdt` crate docs in `../architect/libs/crdt/src/codec.rs` — current helpers + the `LoroRepo` shape.
- Loro project's [text-editor tutorial](https://loro.dev/docs/tutorial/text) — the canonical recipe.
- Lexical's `LexicalNode` model — their split of "doc state" vs "DOM state" is the cleanest description of the pattern we need to emulate.
- assistant-ui's text-editing primitives in `~/Development/research/` — if cloned; otherwise their docs.

## Files this plan will touch

```
../architect/libs/crdt/src/codec.rs                     # Phase 1: new helpers
../architect/libs/crdt/src/codec/tests.rs               # Phase 1: tests

features/knowledge/knowledge-crdt/src/lib.rs            # Phase 2: BlockEntity codec
features/knowledge/knowledge-crdt/Cargo.toml            # Phase 2: if loro re-exports needed
features/knowledge/knowledge-ui/src/editor/input.rs     # Phase 2: TextOp return
features/knowledge/knowledge-ui/src/editor/block.rs     # Phase 2: beforeinput rewrite
features/knowledge/knowledge-ui/src/editor/caret.rs     # Phase 2: minor — ensure UTF-16 offsets
crates/task-ui/src/feature_routes/knowledge.rs          # Phase 2: on_block_text_ops wire
crates/task-ui/src/feature_routes/project.rs            # Phase 2: same wire for the embed

features/chat/chat-crdt/src/lib.rs                      # Phase 3: MessageEntity codec
apps/server/src/chat.rs                                 # Phase 3: SSE streaming → ops
```

## Acceptance criteria

After all three phases:

1. Open `/knowledge` in two browser tabs. Pick the same page in both. Type into the same block from each tab simultaneously. Both peers' keystrokes appear in both tabs. No characters lost. No duplication.
2. Open `/chat-ai`, send a message that triggers a mock streaming response. Observe the assistant body fill character-by-character (not in 1-second batches) in both tabs.
3. `cargo test -p crdt -p knowledge-proto -p knowledge-crdt -p knowledge-ui` all green.
4. `cargo bench -p knowledge-crdt -- text_concurrent` (new bench) shows two-peer 100-edit merge completes in <50ms.

Update README's "What the words mean here" → "Realtime / Collaborative" section once this lands to reflect that text editing is now character-level CRDT, not just per-block LWW.
