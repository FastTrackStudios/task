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

The doc itself is a raw text doc: full file content in the root
`LoroText` container `"content"`; server bookkeeping (the last
flushed sha) in the root `LoroMap` `"meta"`.

## Server: DocRegistry + write-behind + inbound merge

One `vault_collab::VaultCollab` per org, mounted in
`org_layer_router` as the `DocSync` service (and the per-file half of
`DocPresence` via `presence::PresenceRouter` — the fixed org-roster
doc id still routes to the standalone `PresenceHost`).

- **Open/seed** — docs open lazily through the registry factory over
  `crdt::FilePersistence` at `<org>/crdt/<doc_id>/` (file-per-doc
  fits the plain-text ethos; `crdt-seaorm` is the drop-in DB
  alternative if that ever changes). A fresh doc is seeded from the
  file's bytes; a re-opened doc whose file changed while it was
  closed adopts the file by character diff.
- **Write-behind** — a per-doc worker (woken by the doc's root
  subscription, debounced 1 s) writes the doc's text through
  `Backend::put_file(IfMatch::Force)` — the backend's ordinary write
  lock, atomic tmp+rename, and `VaultEvent::Put` broadcast with the
  new sha, so sha-guarded clients, Obsidian, and `subscribe` streams
  observe a perfectly ordinary write.
- **Inbound** — a per-vault listener on the same broadcast channel
  (fed by non-CRDT `put_file` callers *and* the FS watcher, which the
  server now attaches per org) merges external writes into open docs.
  Docs that aren't open are skipped; the next open re-seeds.
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
2. **Non-CRDT writes merge via text diff.** An external write into an
   *open* doc is folded in three-way at character level (ops computed
   from the last-flushed text to the new file text, applied clamped).
   Edits typed concurrently with the external write **interleave but
   are never dropped wholesale**. While a doc is closed/evicted the
   file is authoritative: the next open adopts it.
3. **The sha-conflict banner remains for non-collab clients only.**
   They keep `IfMatch::Sha` conditional writes against whatever sha
   the write-behind last committed, and resolve via Reload/Overwrite
   exactly as before.

Known crash windows (accepted v1, documented in code):

- Server dies inside the 1 s debounce → on reopen, `meta.flushed_sha`
  still matches the file, so the unflushed tail is flushed then.
- Double fault (unflushed edits *and* an external file write while
  the server is down) → the file wins on reopen; the unflushed CRDT
  tail is reverted.

## Client

The vault page (`crates/task/ui/src/pages/vault.rs`) stays sha-first; the
collab path layers on top per open file:

- `open_collab` → keyed `CollabSession` (`crates/task/ui/src/collab.rs`):
  `use_synced_doc_keyed(doc_id)` + `use_presence_channel_keyed(doc_id,
  30_000)`.
- **Takeover**: on the first synced revision the session reconciles
  the editor buffer with the replica (three-way against the session's
  last committed text — handshake-window typing is folded in, peers'
  newer edits win over an untouched buffer), then flips live.
- **Editor → doc**: `Editor`'s `on_transaction` sink (skipping
  `is_remote()` events) → `editor_crdt::changes_to_text_ops` →
  applied to the root `LoroText`.
- **Doc → editor**: every doc revision → `read_text` →
  `editor_crdt::remote_text_to_changes` → a transaction tagged
  `user_event("remote")` (the echo-guard convention both sides
  honor).
- **Autosave** pauses while collab is live (the server write-behind
  owns persistence). If the sync session drops to Offline the page
  tears the session down and falls back to sha saves — a fresh
  replica is opened next time, so offline sha writes can never
  double-apply against a buffered CRDT outbox.
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
- Reconnect re-arm: after a Live → Offline teardown the page re-opens
  collab on the next file open; an automatic retry when the
  connection heals would be nicer.
- `delete_file` / file deletion while a doc is open currently leaves
  the doc authoritative (next flush recreates the file).
