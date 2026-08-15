# Vault feature — current architecture

Status: shipped (`features/vault/`). This doc captures the
**current shape**, not future work. Forward-looking pieces:
`vault-sync-desktop-multiserver.md` (open),
`knowledge-graph.md` (research → planned `vault-graph` crate).

## Layout

```
features/vault/
  vault-proto/      Wire contract. `#[architect::rpc] trait VaultSync`
                    + payload types (Manifest, FileBytes, PutAck,
                    IfMatch, VaultEvent, VaultSyncError). Wasm-clean.
                    Single source of truth for the wire service
                    name (`vault_proto::descriptor().service_name`).
  vault-live/       Implementation. `vault::Backend` impls
                    `VaultSync` over `std::fs` with a watcher +
                    broadcast channel. Two layouts:
                      Backend::single(id, root)
                      Backend::with_roots(map)
                      Backend::under_parent(parent)   ← multi-tenant
                    Hosts the in-memory `Vault` snapshot,
                    `BlockIndex`, debounced filesystem watcher,
                    and the migrated parsers (refs, bases,
                    lexorank, property_schema) that used to live
                    in `knowledge-proto`.
  vault/            Pure facade. Re-exports the wire trait
                    unconditionally + live impl + obsidian helpers
                    behind feature flags (`live`, `obsidian`,
                    `vox`, all default-on). Everyone outside
                    `features/vault/` depends on this crate;
                    siblings talk to each other directly.
  vault-obsidian/   Obsidian translation layer. Helpers (outline,
                    tasks, props, base queries, link index) plus
                    `open_as_vault(path) -> vault::Vault` and
                    `open_as_backend(id, path) -> vault::Backend`
                    that honor `.obsidian/app.json`'s
                    `userIgnoreFilters` + the `.obsidian` /
                    `.trash` / `.git` skip set.
```

## Server mount

`apps/server` constructs `vault::Backend::under_parent(vault_root)`
and mounts it on the `/vox` route as one more
`acceptor_fn` arm:

```rust
name if name == vault_proto::descriptor().service_name => {
    connection.handle_with(vault_proto::serve(vault_sync_state.clone()));
    Ok(())
}
```

`vault_proto::serve(backend)` is the architect-emitted mount
verb — wraps the backend in a `VaultSyncRpcDispatcher` and pulls
its `TokioBlockingDispatcher` via `HasDispatcher`. Wire-level
service name: `"VaultSyncRpc"` (architect's `#[rpc]` macro
suffixes the hidden vox mirror trait).

## Watcher → broadcast wiring

`Backend::start_watcher(vault_id)` attaches a debounced FS
watcher to one registered vault root. External edits (vim,
Obsidian, `git pull`) translate into the same
`vault_proto::VaultEvent`s that PUT/DELETE wire calls emit, and
push onto the broadcast channel `subscribe` reads from.
Forwarder runs on a dedicated OS thread. Dropping the returned
`WatcherHandle` closes the debouncer; the thread exits when
the sender hangs up.

Caveat: clients may see one duplicate event after their own
writes (commit broadcast + watcher echo). Both carry the same
`sha256` so dedupe is trivial.

## Example vault

`examples/vault/` — 139 markdown pages + 5 `.base` files,
tracked in git. Every concept / project / synthesis page
carries explicit `type:` + `sources:` frontmatter so the
future `vault-graph` 4-signal relevance model has shape to
score against. Layout is the 9-folder shape documented in
`examples/vault/README.md`: `Inbox/`, `People/`, `Wiki/`,
`Wisdom/`, `Journal/{Daily,Meetings}/`, `Projects/`, `Task/`,
`Operations/{Locations,Inventory/Pantry}/`,
`Records/bookings/`. `.base` files live colocated with the
data they query (no top-level `Bases/`).

CLI smoke:
```
$ task vault open examples/vault
vault: examples/vault
  pages:       79
  bases:       2
  attachments: 0
  loaded in:   21ms
```

## Test coverage

- 56 vault-live unit tests
- 60 vault-obsidian unit tests
- 3 vault-sync e2e tests (`apps/server/tests/vault_sync_e2e.rs`)

All green; wasm32 web build clean.
