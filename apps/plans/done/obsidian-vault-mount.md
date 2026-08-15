# Obsidian Vault Mount

**Status:** in progress
**Scope:** Open an arbitrary Obsidian vault directory, parse every `.md` file into our existing `Vault`/`Folder`/`Page`/`Block` CRDT entities, render them through the existing `knowledge-ui` shell, and write edits back to disk. Bi-directional sync via `notify-debouncer-mini`. Editor surface stays simple (per-block raw-markdown `<textarea>`) — Editor-crate integration is a separate later arc.

**Test vault:** `/home/cody/Documents/The Observatory` (210 entries, real `.obsidian/` config + plugins + bookmarks + Charts.base).

---

## Why now

The new `Editor/` crate (CodeMirror-style, file-backed) is the future editing surface but not yet wired in. Before we replace the editor, we want the *experience around it* — file-backed vault open, FS as source-of-truth, round-trip safe write-back — to work against real Obsidian vaults. The current shell hard-codes the Logseq `pages/`+`journals/` layout; Obsidian uses an arbitrary tree with `.obsidian/` config.

The CRDT remains the in-memory source of truth at runtime. The on-disk vault is the *durable* source of truth — closing the app and re-opening rehydrates from disk. Loro snapshots become an opt-in *acceleration* layer (block-level merges, multi-peer sync), not the canonical store.

## Reference implementations

| Source | Path | What to copy |
|---|---|---|
| Quartz | `~/Development/research/quartz/` | `quartz/plugins/transformers/ofm.ts` (793 LOC) is the canonical Obsidian-flavored-markdown reference — wikilinks, callouts, highlight, tags, arrows, embeds. `frontmatter.ts` shows YAML/TOML/JSON delimiter handling we don't yet do. `links.ts` shows slug resolution. `processors/parse.ts` shows orchestration. We're not porting — our `knowledge-proto/src/obsidian.rs` already covers most of the grammar — but `ofm.ts` is the canonical correctness reference when our parser disagrees with Obsidian. |
| Logseq | `~/Development/research/logseq/` | Already cited for block model in `plans/logseq-data-model-alignment.md`. |
| Obsidian developer docs | `~/Development/research/obsidian-developer-docs/` | Authoritative on `.obsidian/` config shape, `app.json` keys, ignore filters. |
| Obsidian sample plugin | `~/Development/research/obsidian-sample-plugin/` | API surface — useful when designing plugin-equivalent surfaces later. |

Gaps Quartz handles that our `obsidian.rs` doesn't (capture as `// FUTURE:` markers, do not block phase 1):
- TOML / JSON frontmatter delimiters (`+++`, `;;;`).
- Arrow ligatures (`-->`, `==>`, `<--`, `<==`).
- YouTube / MP3 / PDF embed shorthand inside wiki-embed (`![[video.mp4]]`, `![[song.mp3]]`).

## Background — what we already have

| Piece | Path | Notes |
|---|---|---|
| `Vault` entity | `knowledge-proto/src/lib.rs` + `knowledge-crdt/src/vault.rs` | `root_path: Option<String>`, `attachment_folder_path`, `use_markdown_links`, `new_link_format` already mirror Obsidian config |
| Markdown parser | `knowledge-proto/src/obsidian.rs` (`parse_page`, `ParsedPage`, `ParsedBlock`) | Full Obsidian grammar: frontmatter, callouts, lists, headings, code, block-ids, refs |
| Markdown serializer | `knowledge-proto/src/obsidian.rs` (`serialize_page`) | Round-trip-safe; preserves frontmatter key order via `IndexMap` |
| FS importer | `knowledge-ui/src/graph_loader.rs` (`import_logseq_graph`) | Logseq-shaped (`pages/`+`journals/`); reusable parts: `import_one_file`, `create_blocks_recursive` |
| FS writer | `knowledge-ui/src/graph_writer.rs` (`write_all_pages`, `run_disk_writer_loop`) | Logseq-flavored output; doesn't preserve folder layout |
| Watcher skeleton | `knowledge-ui/src/logseq_shell.rs::run_vault_watcher_loop` | Disabled — no self-write guard; recursive watch incomplete |
| `notify` + `notify-debouncer-mini` | `knowledge-ui/Cargo.toml`, native target only | Already pinned |

## What changes

### New crate: `features/knowledge/knowledge-fs`

Native-only sibling of `knowledge-ui`. Keeps the wasm graph clean and gives us one place for FS concerns. Depends on `knowledge-proto`, `knowledge-crdt`, `crdt`, `notify`, `notify-debouncer-mini`, `walkdir`, `tokio`. **No** `dioxus` dep — pure logic.

Module layout:

```
knowledge-fs/
  src/
    lib.rs        Public surface; re-exports below + `VaultHandle`
    walker.rs     Walk a vault root, yield `(rel_path, absolute_path, is_attachment)`. Honors Obsidian ignore rules
    config.rs     Read `.obsidian/app.json` / `appearance.json` / `bookmarks.json` into a typed `ObsidianConfig`
    import.rs     `import_obsidian_vault(doc, root) -> ImportStats` — drives walker + parse_page + creates Folder/Page/Block
    writer.rs     `write_page_to_disk(root, page, blocks)` + `run_disk_writer_loop` (Loro subscribe → debounce → flush touched pages)
    watcher.rs    notify-debouncer-mini loop. SelfWriteGuard de-dupes echo events
    mount.rs      `VaultHandle` — owns writer-task + watcher-task JoinHandles; `drop` aborts both
```

### Obsidian-specific behavior the walker handles

- Skip top-level `.obsidian/`, `.trash/`, `.git/`, `.DS_Store`.
- Honor `userIgnoreFilters` from `.obsidian/app.json`.
- Preserve folder hierarchy → create `Folder` entities. `Page.path` = vault-relative path with extension (e.g. `Music/Charts.md`).
- Identify attachments (anything in `attachmentFolderPath` or any non-`.md` file referenced by a page). v1: record their relative path on the page; don't move them. v2: copy / link assets.
- Excalidraw `.excalidraw.md` already handled by `graph_loader` — port behavior.
- `.base` files (Obsidian Bases) — skip in v1 with a warning; Bases is a separate feature.
- Canvas `.canvas` files — skip in v1.

### Self-write guard

The watcher echo problem: writer writes file → watcher fires → reimports → marks blocks dirty → writer fires again. Guard:

```rust
struct SelfWriteGuard {
    recent: Mutex<HashMap<PathBuf, Instant>>,
}
impl SelfWriteGuard {
    fn mark_written(&self, p: &Path) { recent.insert(p.into(), Instant::now()); }
    fn is_recent_self_write(&self, p: &Path) -> bool {
        recent.get(p).map(|t| t.elapsed() < Duration::from_millis(1500)).unwrap_or(false)
    }
}
```

Writer calls `mark_written` immediately before / after `fs::write`. Watcher skips events whose path is `is_recent_self_write`. 1500ms window absorbs the OS-level write → flush → mtime-update latency on Linux/macOS.

### Editor swap

Replace the contenteditable inside `knowledge-ui/src/outliner.rs` (and any siblings — `live.rs`) block editor with a `<textarea>` showing the raw markdown content of `Block.content`. On `oninput`, debounce 250ms and call `BlockRepoLoro::update`. No live-preview rendering of the editing block. Sibling (non-editing) blocks still render through the existing `BlockView` inline-markdown renderer.

Scope is **explicitly not** to ship a great editing experience — it's to ship a *correct* editing experience so we can validate the vault round-trip. The real editor swap is the `Editor/` crate arc.

### UI mount

New "Open Obsidian Vault" entry in the existing logseq_shell sidebar (rename later). Uses `rfd::AsyncFileDialog::pick_folder()` to choose root. Persists last-opened path in a `vault.lastOpened` key inside `app.json`-style local config (or simply a `VaultRepo` field). On startup, if last-opened exists and is still a directory, auto-mount; else show empty state with "Open Vault" CTA.

When a vault is mounted, the shell:
1. Wipes the in-memory CRDT (`CrdtDoc::ephemeral()`).
2. Runs `import_obsidian_vault`.
3. Spawns `run_disk_writer_loop`.
4. Spawns `run_watcher_loop`.

Closing a vault: abort both loops via `VaultHandle::drop`, reset doc.

## Sequencing

**Phase 1 — Walker + import (this PR)**
- Create `knowledge-fs` crate (lib.rs, walker.rs, import.rs, config.rs).
- `walker::walk_vault(root)` yields entries.
- `config::read_obsidian_config(root)` parses `app.json` + `appearance.json` (best-effort, missing files = defaults).
- `import::import_obsidian_vault(doc, root)` populates Vault + Folder + Page + Block.
- Unit tests: parse synthetic vault under `tempdir`. Manual smoke: cargo test against `~/Documents/The Observatory`.

**Phase 2 — Writer + watcher**
- `writer::write_page_to_disk` using `obsidian::serialize_page`.
- `writer::run_disk_writer_loop` subscribes Loro local updates, debounces, flushes dirty pages only (track via `(page_id, updated_at)` cache; not every-page rewrite like `graph_writer`).
- `watcher::run_watcher_loop` recursive watch + self-write guard.
- Round-trip test: import → edit → assert disk content → external edit → assert CRDT picks up.

**Phase 3 — UI mount + editor swap**
- Add Open Vault button, picker, mount flow.
- Swap block editor for raw textarea.
- Smoke test against The Observatory.

**Phase 4 — Polish (out of immediate scope, captured for next plan)**
- Attachment copy + ref rewriting.
- `.base` and `.canvas` round-trip.
- Bookmarks → architect-ui pinned-pages sidebar section.
- Conflict UI when external edit races a local one.
- Editor-crate integration.

## Acceptance criteria

1. `cargo check -p task-ui` + `cargo check -p task-app-web --target wasm32-unknown-unknown` both clean. Wasm graph must not pull in `knowledge-fs`.
2. Mounting `/home/cody/Documents/The Observatory` populates the sidebar with 210 entries in their original folder hierarchy.
3. Editing a block in the UI writes the corresponding `.md` file within 1s, byte-equal except for the edited content. Frontmatter key order preserved. No spurious whitespace churn.
4. External edit to a `.md` file (e.g. `vim` save) reflects in the UI within 2s. No re-import storm — exactly one reimport per debounced burst.
5. Closing + reopening the app rehydrates the same vault from disk.

## Risk register

| Risk | Mitigation |
|---|---|
| Write-storm via watcher echo | `SelfWriteGuard` with 1.5s window. Diagnose via `tracing::info!` per write and per ignored event. |
| Block ID drift on re-import | Honor `^block-id` anchors → deterministic UUID derivation. Plain blocks: keep stable order via `sort_key`, reuse existing block-id if `(page_path, content_hash, sort_key)` matches. |
| `serialize_page` introduces unintended diff | Round-trip test: parse a file, serialize, assert byte-equal where possible. If not byte-equal, document the legitimate normalizations. |
| `userIgnoreFilters` regex syntax | Obsidian uses simple glob-style; v1 supports prefix + suffix only, falls back to ignoring unrecognized patterns with a warning. |
| Concurrent edit conflict (FS vs in-app) | v1: last-writer-wins (debounce ordering decides). v2: surface in UI with diff resolution. |
| Wasm leakage | `knowledge-fs` is native-only via `[target.'cfg(not(target_arch = "wasm32"))']` AND not pulled by `task-app-web`. Guard with the existing wasm cargo-check. |

## Out of scope

- Obsidian community-plugin support (Tasks, Dataview, etc.).
- Obsidian Sync / Obsidian Publish replacement.
- Replacing the editor with `Editor/` crate — that's a separate plan.
- `.base` view rendering.
- Loro opt-in for collaborative block-level editing on a vault. Path: when a vault is mounted with collab enabled, add a `<vault>.loro` snapshot file alongside; sync layer can re-create from it. Marked for the *decentralized-foundation* arc.
