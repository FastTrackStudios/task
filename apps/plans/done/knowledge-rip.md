# Knowledge feature rip — record

Shipped as commit `64a8740`. ~140 insertions / 11,267 deletions.

## Why

The `knowledge` feature was three things bolted together:
1. **Parsers** (`.base` query DSL, frontmatter, refs, lexorank,
   property_schema) — Layer-1 pure functions vault uses.
2. **CRDT entity model** (Page / Block / Folder / Vault / Tag /
   Base as Loro entities) — alternative-to-files storage path.
3. **UI** (Logseq-style outliner shell, properties pane,
   tasks-kanban) — legacy editor surface.

The user's direction: file-backed vault is the only storage
path. CRDT lives only at the (future) per-file editor layer.
Knowledge's entity-CRDT layer + Logseq-style UI both go away;
parsers migrate into the vault feature.

## What moved

| From | To |
|---|---|
| `knowledge-proto::refs` | `vault-live::refs` |
| `knowledge-proto::bases` | `vault-live::bases` |
| `knowledge-proto::lexorank` | `vault-live::lexorank` |
| `knowledge-proto::property_schema` | `vault-live::property_schema` |
| `knowledge-proto::obsidian` (frontmatter parser) | `vault-obsidian::obsidian_parse` |

`obsidian_parse` had to strip 4 entity-coupled functions during
the move: `resolve_linkpath`, `serialize_page`,
`translate_logseq_block_refs`, `format_link` — they all
returned or accepted the deleted `knowledge_proto::{Page,
Block, Vault}` entity types. Rebuild against
`vault_obsidian::{VaultPage, Vault}` if they come back.

## What was deleted outright

- `features/knowledge/knowledge-proto/` (the whole crate)
- `features/knowledge/knowledge-crdt/` (the whole crate)
- `apps/publish/` + `crates/publish-core/` (publish read
  `.loro` snapshots — input format gone; publish-core parked
  under `crates/legacy/publish-core/`)
- `apps/server/src/{knowledge_index, basename_index}.rs`
- `apps/server/tests/{knowledge_e2e, knowledge_indexes,
   markdown_roundtrip, property_schema}.rs`
- 6 server `*RepoLoro` dispatcher arms (VaultRepo /
  FolderRepo / PageRepo / BlockRepo / KnowledgeTagRepo /
  BaseRepo)
- Server's org-vault doc pre-open + the indexer
  `subscribe_local_update` hook + `MemoryBasenameIndex`
- Server's `seed_knowledge_org_vault` /
  `seed_formatting_demo_idempotent` /
  `seed_formatting_demo` boot-time seeders
- CLI `task export` + `task import` subcommands + their
  helpers

## What was parked under `crates/legacy/`

- `knowledge-ui/` — Logseq-style shell, properties pane,
  tasks-kanban, awareness, idb-persistence (~18K LOC).
  Kept for mining: virt-folder views, properties pane,
  tasks-kanban patterns.
- `publish-core/` — markdown rendering engine. Kept because
  the future static-site generator (if rebuilt) can crib
  from it.
- `editor-outliner/` — older Logseq-style outliner that
  predated the CodeMirror-style editor. Parked since
  `task-ui` now mounts `editor::EditorApp` instead.
- `project-ui/` — added later in the project-CRDT rip
  (`63ce4a1`); its `TasksByProjectLive` component mined task
  rows out of knowledge pages.

Legacy crates are NOT in the workspace member list, so
`cargo check --workspace` doesn't try to compile them. Add
them back individually when mining a piece.

## Verification

- 56 vault-live + 60 vault-obsidian + 3 vault e2e tests pass
- `cargo check --workspace --exclude playground` clean
  (native)
- `cargo check -p task-app-web --target wasm32-unknown-unknown`
  clean inside `nix develop`

## Follow-up that landed shortly after

The CLI's `task export` / `task import` / `task list` /
`task set-done` / `task new-task` / `task new-project`
commands were CRDT-backed and went away with the
`knowledge-crdt` rip. Replacement landed in the
`features/task/task` slice (`bfd68ea`) — see
`plans/task-feature.md`.
