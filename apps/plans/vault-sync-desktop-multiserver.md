# Desktop: multi-server vault wiring

**Status:** not started. The client crate + server endpoints are done and e2e-covered; the desktop wiring this plan describes is the open part.

Follow-up to the vault-sync slice
(`apps/server/src/vault_sync.rs`, `crates/vault-sync`). The
client crate and server endpoints are done and covered by
e2e tests; this plan is about wiring real desktop UX on top.

## Goal

Desktop app can hold **multiple** active vault connections at
once, mixing two kinds:

- **Local** — direct filesystem access through the
  `vault::Vault` snapshot + watcher. Used when desktop runs
  on the same machine as the server (or just standalone, no
  server at all). This is what we test today.
- **Remote** — HTTP + WS via `vault_sync::VaultClient`. Used
  when desktop sits on a different machine than the server,
  or when a single user wants to mirror two servers from one
  app window.

A user with a laptop and a home server has both Local
(`~/Documents/Task`) and Remote (`https://my-server`) open
at the same time and can switch between them.

## Existing pieces to extend

- [`crates/task-ui/src/server_registry.rs`](../crates/task-ui/src/server_registry.rs)
  already persists multi-server entries (`ServerEntry { id,
  label, server_url, session_token, my_user_id }`). Currently
  vox-oriented. Extend with a `kind` discriminator.
- [`crates/vault/src/vault.rs`](../crates/vault/src/vault.rs)
  is the in-memory snapshot for the local case.
- [`crates/vault-sync-proto/src/lib.rs`](../crates/vault-sync-proto/src/lib.rs)
  defines the `VaultSync` vox service. The remote backend wraps
  the architect-emitted `VaultSyncClient` connected over vox —
  same client type used on native and wasm.

## Sketch

```rust
enum VaultBackend {
    Local  { path: PathBuf, vault: Arc<RwLock<vault::Vault>> },
    Remote { client:   vault_sync_proto::VaultSyncClient,
             vault_id: String,
             cache:    Arc<RwLock<vault::Vault>> }, // optional mirror
}

struct ServerEntry {
    id: Uuid,
    label: String,
    backend: VaultBackend,
    // existing auth fields stay only for remote
    session_token: Option<String>,
    my_user_id: Option<Uuid>,
}
```

Open questions:

1. **Remote-side cache**: do we hydrate a `vault::Vault`
   from the manifest on connect (so the editor sees the
   same shape as local), or does the editor learn to talk
   to `VaultClient` directly? Caching keeps the editor
   uniform but means every remote vault gets a full disk
   mirror — fine for desktop, not for mobile.
2. **Write path**: edits go to disk for local; for remote
   they go to `put_file` with the last-known sha as
   `IfMatch::Sha(_)`. The cache mirror is updated when the
   WS subscriber emits the matching `Put` event (so the
   server stays the conflict arbiter).
3. **Sidebar / picker UI** lives in `task-ui` — needs a
   server picker chrome (currently the registry's there
   but the UI assumes a single active server in places).

## Slice plan

1. Extend `ServerEntry` with `kind: Local | Remote`. Make
   the existing remote shape default. Migrate any callers.
2. Add a `VaultBackend` enum (likely in a new
   `crates/vault-host` crate, or as a module on `task-ui`)
   that wraps either `vault::Vault` or
   `vault_sync_proto::VaultSyncClient`.
3. Wire a single Local backend into the desktop launcher,
   pointing at `~/Documents/Task`. This is functionally a
   no-op for current users (same as today) but exercises
   the new abstraction.
4. Add the remote variant + a "Connect to server" dialog.
   Use the existing auth flow stub from `server_registry`
   even though vault-sync itself doesn't enforce auth yet
   — the server bearer token will plug in here later.
5. Watch the remote subscription, refresh the editor's
   open page when its `rel_path` shows up in a Put event.

## Out of scope

- Web / wasm transport — no longer a separate concern.
  `vault_sync_proto::VaultSyncClient` builds for both native
  and wasm already; the same `VaultBackend::Remote` shape
  works on the web app once it adopts it.
- Cross-server links (a `[[Page]]` in vault A pointing at
  vault B). Solvable later via per-server prefixes; ignore
  it for now.
- Per-server encryption-at-rest. We rely on TLS in transit
  and OS-level disk encryption for now.
