# Sync architecture

## The shape we're building toward

Task is a **federated vault** — every device holds the full
markdown corpus locally; a central server is the canonical
truth for sync but never the only copy. Devices stay usable
offline; reconciliation happens on reconnect.

- **Markdown files** (`.md`, `.base`, `.canvas`) replicate to
  every device.
- **Large media** (images, audio, video, PDF) stays on the
  server with cheap on-demand fetch — local-only would
  balloon mobile storage.
- **Settings + indexes** are device-local. Each device
  rebuilds `MetadataCache` / `BlockIndex` from its own copy
  of the vault. Matches Obsidian's "no persistent DB" model.

## Phases

### Now (this branch)

- `vault` crate is the on-device file model. Loads the vault
  from disk, builds a block index, watches for change.
- Each device runs the same `vault::Vault::open` flow with
  no awareness of other devices. The vault on disk IS the
  state.

### Next: sync server (no CRDT)

Modeled on
[`vrtmrz/obsidian-livesync`](https://github.com/vrtmrz/obsidian-livesync):
- File-level sync. Server is a thin store keyed by
  `(vault_id, rel_path)` → blob + mtime + hash.
- Devices push on save, pull on connect + on push
  notification.
- Conflict resolution = file-level "last writer wins" with
  conflict markers when a file changed on both sides. Same
  shape as iCloud / Dropbox.
- No structural awareness of markdown content. The server
  doesn't know about blocks, headings, properties.

This phase is "good enough" for multi-device single-user.
Most of our users will live here for a while.

### Later: live collab via CRDT

Loro lands when we have a clear use case (live multi-user
editing on a single block, presence, comments). Block-level
granularity is the natural unit:

- Each block has a Loro doc keyed by its UUID.
- Loro syncs through the same server; the server stays thin
  (forwards ops by topic).
- Markdown file format stays the source of truth on disk;
  CRDT state is rebuilt from the markdown on first edit and
  garbage-collected when the block returns to a quiescent
  state.

Scope-wise this is the big one — design it when projects /
multi-user collaboration is the actual focus, not before.

## Implications for what we build today

- Every change made by the editor goes through the file on
  disk. No "write to memory, sync later" — the file IS the
  message we'd sync.
- Reads happen against an in-memory snapshot (`Vault`),
  built once at startup + refreshed by the watcher. Same
  pattern Obsidian uses; same pattern the sync server will
  push diffs into.
- `vault::Vault` deliberately mirrors `obsidian-compat`'s
  shape so a future sync layer can sit between them or under
  them without the editor needing to know.
- Property-type hints (`.obsidian/types.json`) and `.base`
  files travel as part of the vault. Anything that lives on
  disk is in scope; anything in the runtime cache is not.
