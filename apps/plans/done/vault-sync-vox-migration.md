# Vault-sync architect::rpc migration — SHIPPED

Status: **done.** Kept as the design record for why the
file-replication layer rides on architect::rpc (and thus vox)
like every other feature.

## Crate layout

```
features/vault/
  vault-proto/      wire contract (#[architect::rpc] trait VaultSync,
                     Manifest, FileBytes, PutAck, IfMatch, VaultEvent,
                     VaultSyncError). Wasm-clean. Single source of
                     truth for the wire service name (consumers use
                     `vault_proto::descriptor().service_name`).
  vault/            primary backend impl. `vault::Backend` impls
                     VaultSync + HasDispatcher (TokioBlockingDispatcher).
                     Two layout modes:
                       Backend::single(id, root)
                       Backend::with_roots(HashMap<id, root>)
                       Backend::under_parent(parent)   ← multi-tenant,
                                                          auto-create
                     Also home of the in-memory Vault snapshot +
                     BlockIndex + watcher (unchanged).
  vault-obsidian/   Obsidian translation / import layer (renamed
                     from `obsidian-compat`). Not a VaultSync
                     backend on its own — exists so `vault` (or
                     CLI consumers) can understand Obsidian-shaped
                     directories: skip `.obsidian` / `.trash`,
                     parse `.base` files, frontmatter conventions,
                     wikilink index, tasks, properties, outline,
                     wordcount, base queries. The "open as Task
                     vault" entry points are
                     [`vault_obsidian::open_as_vault`] and
                     [`vault_obsidian::open_as_backend`] — both
                     honor `userIgnoreFilters` from
                     `.obsidian/app.json` and produce the
                     canonical `vault::Vault` / `vault::Backend`
                     types so the rest of the stack doesn't care
                     where the directory came from.
```

## What shipped

- `features/vault/vault-proto` — one canonical sync trait
  `VaultSync` decorated with `#[architect::rpc]`. Sync CRUD
  methods (`manifest`, `get_file`, `put_file`, `delete_file`)
  + async `subscribe(Tx<VaultEvent>)` (mixed-mode trait).
  Borrowed `&str` args in the sync signature; the macro
  rewrites to owned `String` for the async client mirror.
  Payload types (`Manifest`, `ManifestEntry`, `FileBytes`,
  `PutAck`, `IfMatch`, `VaultEvent`, `VaultSyncError`) are
  `#[derive(Facet)]` + `vox_types::Reborrow`, split per-domain
  (`manifest.rs` / `file.rs` / `event.rs` / `error.rs` /
  `service.rs`). Wasm-clean.
- `vault::Backend` is the **only** `VaultSync` implementation.
  It impls `HasDispatcher` returning
  `TokioBlockingDispatcher` so remote sync calls run inside
  `spawn_blocking`. The server constructs it with
  `vault::Backend::under_parent(vault_root)` and mounts it
  via the architect-emitted
  `vault_proto::serve(backend)` verb as one more arm in
  `vox_ws_handler` alongside `ProjectRepo` / `WorkspaceSync`
  / `AttachmentService` / etc. The match arm matches against
  `vault_proto::descriptor().service_name` (currently
  `"VaultSyncRpc"`, suffixed by architect's `#[rpc]` macro on
  the hidden vox mirror trait). No separate REST routes; no
  second WS upgrade. The previous server-local
  `vault_sync::VaultSyncState` was deleted — same logic now
  lives in `vault::Backend`.
- The old `crates/vault-sync` native HTTP client crate has been
  deleted. Consumers use the architect-emitted
  `vault_proto::VaultSyncClient` directly — same client builds
  for native (tests, desktop) and wasm (`apps/web`) because
  `vox` itself is target-agnostic. This obsoletes the
  separate `vault-sync-web-transport` plan.
- `apps/server/tests/vault_sync_e2e.rs` drives the real
  `VaultSyncClient` against a booted `task-server`. Three
  cases: `put → manifest → get`, `subscribe` stream observing
  PUT + DELETE, and the conflict round-trip carrying server
  bytes + sha inside `VoxError::User(Conflict)`.

## Notes

File bytes through vox encode as `Vec<u8>` inside `FileBytes` /
`PutFileArg`. Fine for markdown pages; large media still belongs
in the `attachments` flow (signed-URL HTTP PUT/GET), unchanged
by this migration.

The original "vox is RPC-shaped, our events are topic-shaped"
doubt didn't survive contact with the codebase: `vox::Tx<T>`
return-by-output-channel handles topic-style streaming cleanly
— see `WorkspaceSync::subscribe` and now `VaultSync::subscribe`.

Using `#[architect::rpc]` instead of `#[vox::service]` (the
shape daw uses for its 11+ ported services) means we get the
sync trait + async client mirror from one declaration: the
trait reads as plain sync code (`fn put_file(&self, vault_id:
&str, …)`), backends impl it directly with zero call-site
ceremony, and the architect bridge marshals each call through
the backend's `Dispatcher` for cross-process callers. No
wrapper arg structs (no more `PutFileArg` / `VaultIdArg`); the
macro rewrites the borrowed args to owned for the wire form.

## Out of scope (still open)

- Desktop multi-server wiring (Local vs Remote backends) —
  `plans/vault-sync-desktop-multiserver.md`. The new
  `VaultSyncClient` is what the `Remote` variant wraps.
- Per-vault encryption-at-rest — deferred; TLS in transit +
  OS-level disk encryption only.
