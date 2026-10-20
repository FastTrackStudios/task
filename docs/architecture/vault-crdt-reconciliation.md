# Vault-file ⇄ CRDT reconciliation

How a plain-markdown vault and real-time collaborative editing share
one source of truth. Code: `features/task/vault/vault-collab` (server),
`crates/task/ui/src/collab.rs` (client),
`features/task/vault/vault-proto/src/collab.rs`
(identity scheme). Tracked issue: `cfa1c33b`.

## Identity: one doc per (vault, path)

Every collaboratively-edited vault file maps to exactly one Loro doc,
addressed by **UUID v5 of `(vault_id, path)`** under the fixed
namespace `task-vault-docid` (`vault_proto::VAULT_DOC_NAMESPACE`,
helper `vault_proto::collab_doc_id`). The id is a pure function —
stable across restarts, machines, transports; no allocation table.

- Clients obtain it via the `VaultSync::open_collab(vault_id, path)`
  RPC, which validates the path exists and registers the
  `doc_id → (vault_id, path)` reverse mapping the server's
  `DocRegistry` admission hook consults (the hook only ever sees a
  `Uuid`).
- **Renames move content to a new doc id** (v1). A renamed file is a
  new `(vault_id, path)`; the old doc's history stays parked under
  the old id. Carrying history across renames needs a rename event on
  the `VaultSync` wire (the watcher reports renames as
  delete + create today) — separate issue.

The doc has the **document shape** (`offline.doc.shape`, since
2026-10-13): the markdown body in the root `LoroText` `"body"`, the
frontmatter's top-level fields in the root `LoroMap` `"props"` (plain
values, so a property merges by key); server bookkeeping (the last
flushed sha, and the doc version the mirror was rendered from) in the
root `LoroMap` `"meta"`. The codec between this shape and markdown is
`features/vault/vault-doc` — one conversion for the server mirror, the
editor buffer, a desktop folder and export, byte-stable for unchanged
documents. Docs made before the split (whole file in `"content"`) are
converted once on open (`vault_doc::migrate_whole_file`); a client still
typing into `"content"` changes nothing rendered.

`open_collab` also accepts a path with **no file yet** — that is how a
document created on a device offline (`offline.doc.create`) reaches the
server; its first content's flush creates the file.

## Server: DocRegistry + write-behind + inbound merge

One `vault_collab::VaultCollab` per org, mounted in
`org_layer_router` as the `DocSync` service (and the per-file half of
`DocPresence` via `presence::PresenceRouter` — the fixed org-roster
doc id still routes to the standalone `PresenceHost`).

- **Open/seed** — docs open lazily through the registry factory over
  `crdt::FilePersistence` at `<org>/crdt/<doc_id>/` (file-per-doc
  fits the plain-text ethos; `crdt-seaorm` is the drop-in DB
  alternative if that ever changes). A fresh doc is made from the
  file; a re-opened doc whose file changed while it was closed merges
  the edit **as a change**, three-way against the version the mirror was
  rendered from (`meta.flushed_at`) — the doc's own unflushed edits stay,
  and the mirror is then rewritten from the merged doc.
- **Write-behind** — a per-doc worker (woken by the doc's root
  subscription, debounced 1 s) renders the doc (`vault_doc::render`,
  keeping the last file's unchanged frontmatter lines) and writes it through
  `Backend::put_file(IfMatch::Force)` — the backend's ordinary write
  lock, atomic tmp+rename, and `VaultEvent::Put` broadcast with the
  new sha, so sha-guarded clients, Obsidian, and `subscribe` streams
  observe a perfectly ordinary write.
- **Inbound** — a per-vault listener on the same broadcast channel
  (fed by non-CRDT `put_file` callers *and* the FS watcher, which the
  server now attaches per org) merges external writes into open docs
  (`vault_doc::merge_text`: changed props by key, the body's edit by
  character, committed under an `"outside edit"` message). Docs that
  aren't open are skipped; the next open merges the edit as above.
- **Attach guard** (`apps/server/src/doc_guard.rs`) — every `DocSync`
  attach asks the file's write guard for the caller, and refuses a
  replica whose history shares no peer with the served doc
  (`vault_doc::FOREIGN_LINEAGE`: the server's doc was recreated — a fresh
  data root, a reseeded demo — and merging would duplicate every line).
- **Echo guard** — the write-behind records the flushed sha (and
  text) *before* `put_file`, in memory and in the doc's `meta` map;
  the inbound listener drops events whose sha matches. The `meta`
  copy survives restarts, which is how a re-opened doc distinguishes
  "my last flush is still current" from "the file changed while I was
  away".
- Registry hygiene: compaction every 64 updates, idle eviction after
  15 min (compact + drop; reopen re-seeds). All subscription closures
  follow the WeakCrdtDoc rule — no strong doc handle ever lives
  inside a loro callback.

## Conflict policy

1. **CRDT wins between collaborating clients.** Concurrent edits from
   synced replicas merge by Loro semantics; there is no conflict
   banner on the collab path.
2. **The document is the truth; text coming in is a change.** An
   external write is merged three-way — props by key, the body at
   character level — against the version it was made from (the last
   flushed text for an open doc, `meta.flushed_at` for a closed one).
   Edits made concurrently **interleave but are never dropped
   wholesale**, and the file is then written from the document. A
   frontmatter that does not parse is read as body, so no text is lost.
3. **The sha-conflict banner remains for non-collab clients only.**
   They keep `IfMatch::Sha` conditional writes against whatever sha
   the write-behind last committed, and resolve via Reload/Overwrite
   exactly as before.

Known crash windows (accepted v1, documented in code):

- Server dies inside the 1 s debounce → on reopen, `meta.flushed_sha`
  still matches the file, so the unflushed tail is flushed then.
- Double fault (unflushed edits *and* an external file write while
  the server is down) → closed since 2026-10-13: the reopen merges the
  file's edit into the doc against `meta.flushed_at`, keeping both.

## Client

The vault page (`crates/task/ui/src/pages/vault.rs`) stays sha-first; the
collab path layers on top per open file:

- Keyed `CollabSession` (`crates/ui/src/collab.rs`) over
  `crate::doc_replica::use_replica_into` — the file's document as a
  replica **persisted on the device** (`vault_doc::Replica` over the
  device database's `crdt::Persistence`; IndexedDB on the web), with one
  sync session kept alive (dial, `open_collab`, attach) and retried with
  backoff when the connection drops — nothing is torn down offline —
  plus `use_presence_channel_into(doc_id, 30_000)`. The doc id is
  derived on the device; no round trip is needed to open a replica.
- **Takeover**: once the replica holds a version worth editing (stored
  on this device before, or the server's merged in) the session
  reconciles the editor buffer with it (three-way against the session's
  last committed text — handshake-window typing is folded in, newer
  edits win over an untouched buffer), then flips live.
- **Editor → doc**: the buffer is the whole file; `on_transaction`
  (skipping `is_remote()` events, and holding back a half-typed
  frontmatter) → `vault_doc::merge_text(doc, pushed, buffer)`.
- **Doc → editor**: every doc revision → `vault_doc::render(doc,
  Some(buffer))` → `editor_crdt::remote_text_to_changes` → a
  transaction tagged `user_event("remote")`.
- **Autosave and explicit saves** stay off for as long as the replica
  owns the buffer — online or offline, focused or not — so a buffer the
  replica carries is never also written as a whole file (the
  double-apply the old Live → Offline teardown existed to prevent).
- **What is not synced** is tracked honestly: the server echoes every
  update it applies, so the union of its version vector at attach and
  the ends of every update received is what it holds; the status line
  and the phone strip say "Not synced" while the replica holds more.
- A replica refused for foreign lineage is reset and the person's
  buffer folded into the server's document.
- **Presence cursors**: `{name, anchor, head}` (scalar units) under a
  per-session key, debounced 200 ms, published on every transaction
  (selection-only included); rendered as decorations (selection
  `Mark` + caret `Widget` with the peer's name, per-peer hue from the
  key hash) layered over the vault decoration pass.

## Follow-ups

- Rename-aware doc identity (needs a wire-level rename event).
- Editor-repo Playwright coverage for typing-during-remote-insert and
  IME composition over the collab path (the seams live in the Editor
  repo; this repo's coverage is the `vault_collab_e2e` convergence
  suite).
- ~~Reconnect re-arm~~ — closed 2026-10-13: there is no teardown; the
  replica driver re-attaches by itself.
- `delete_file` / file deletion while a doc is open currently leaves
  the doc authoritative (next flush recreates the file). Since
  `open_collab` accepts missing paths, a device holding a stale replica
  of a deleted note can bring it back on reconnect — deletion needs to
  be a document-level tombstone.
- Peer sync (`offline.peers`): node ↔ node document sync over iroh —
  see `docs/spec/unmet.md` § Offline — the text side.
