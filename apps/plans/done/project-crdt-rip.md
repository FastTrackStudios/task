# Project-CRDT rip — record

Shipped as commit `63ce4a1`. 48 files changed, 163 insertions,
**6,683 deletions**. The architectural through-line — vault is
the storage path, everything else is files — is finally
honest.

## Why

After the `knowledge-rip` removed the structured-vault Loro
layer, the only remaining CRDT-backed code was the
`project-{proto, crdt}` pair driving the server's
`ProjectRepo` / `TaskRepo` / `WorkspaceSync` dispatchers.

The user's framing: CRDT now lives only at the (future)
per-file editor layer. Tasks + projects = markdown files;
there's no reason to also model them as Loro entities. So
the entire entity-CRDT machinery goes away.

## What was deleted

### Crates
- `features/project/{project, project-proto, project-crdt, project-db}`
- `apps/db` (task-db — the SeaORM Loro persistence)

### Server-side
- `apps/server/src/lib.rs` collapsed from ~990 LOC to ~210 LOC.
  Removed:
  - `DocRegistry` (~85 LOC) — LRU registry of open `CrdtDoc`s
  - `OpenDoc` (~75 LOC) — per-doc state + broadcast channel
  - `WorkspaceSyncImpl` (~360 LOC) — subscribe / apply_update /
    subscribe_kinds / awareness plumbing
  - `UpdateFrame`, `ErasedPersistence` plumbing
  - The workspace-doc pre-open + indexer hook
- `AppState` shrunk to `{auth, keypair, attachments, vault_sync}`.
- Dispatcher arms culled from 9 to 3: `AuthService`,
  `AttachmentService`, `VaultSyncRpc`. Gone: 6 `*Repo` arms,
  `WorkspaceSync`, `ShareService`, `AnonymousClaim`.
- `apps/server/src/main.rs` no longer opens a SeaORM database
  or runs CRDT migrations — auth's sqlite (handled internally
  by architect-auth) is the only DB the server touches.
- Server modules deleted: `acl.rs`, `anonymous_claim.rs`,
  `share_link.rs`.
- `capability.rs` rewritten to hold just `ServerKeypair` +
  `CapabilityError` — the bits the attachments signed-URL
  flow needs. The previous `CapabilityScope` + `DocId`
  fencing surface was project-proto-coupled and went away.
- Server tests deleted: `sync`, `stress`, `federation`,
  `per_kind_sync`, `doc_isolation` (all CRDT e2e),
  `attachments` (project-proto coupled), `capability`,
  `share_link`. Surviving: `auth`, `vault_sync_e2e`
  (rewritten to drop the persistence arg).

### Client + UI
- `apps/cli`: removed `task list` / `task set-done` /
  `task new-task` / `task new-project` (CRDT-backed). Surface
  is now `task doctor` + `task vault <cmd>`. `apps/cli/src/
  shared.rs` shrunk from `LiveSession` + `ServerRegistry` +
  `RemoteVoxConfig` to just `RemoteVoxConfig`.
- `crates/task-ui/src/feature_routes/federated_tasks.rs`
  deleted (CRDT-backed). The `/projects` route is a stub
  pending rebuild.

### Workspace
- Dropped deps: `project`, `project-proto`, `project-crdt`,
  `project-db`, `task-db`. `loro`, `loro-internal`, `crdt`,
  `crdt-seaorm` workspace definitions kept (used by external
  paths like architect / architect-auth) but no in-tree
  consumers.
- `features/*/tests/native` glob commented out.
- `sea-orm` patch retained — architect-auth uses sea-orm for
  its sqlite auth tables.

## What got parked

`crates/legacy/project-ui/` — its `TasksByProjectLive`
component mined task rows out of knowledge pages and broke
when the entities went away. Kept for mining when the
`/projects` route is rebuilt against the file-backed task
feature.

## Verification

- 56 vault-live + 60 vault-obsidian + 3 vault e2e + 1 auth
  test pass (120 total)
- `cargo check --workspace --exclude playground` clean
  (native)
- `cargo check -p task-app-web --target wasm32-unknown-unknown`
  clean inside `nix develop`
- `task vault open examples/vault` still loads 79 pages /
  2 bases in 22 ms

## Follow-up

`features/task/task/` (commit `bfd68ea`) rebuilt task/project
management as file-native pages with TaskNotes-shape
frontmatter. See `plans/task-feature.md` for what's shipped
and what's queued.
