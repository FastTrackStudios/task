# Email Client

Build a fully fledged, local-first email client into Task. Modeled on the
vault feature: one `#[architect::rpc] trait EmailSync` wire contract in
`email-proto`, multiple backend crates implementing it
(`email-imap`, `email-jmap`, `email-maildir`, `email-nextcloud`), and a
local cache (`email-store`) that re-exposes the same trait so callers
can talk to disk the same way they talk to a live server.

## Status

Planning + scaffold. Existing `crates/task-core/src/email/` is the
**reference** type (`EmailRef` — pointer that lives on Task/Project
frontmatter); kept as-is. The `features/email/` tree is the client.

## Goals

- Full-featured client: list / read / compose / reply / forward / search /
  threads / attachments / signatures / drafts / archive / labels.
- Offline-first. Mailbox = on-disk Maildir. UI reads from disk + SQLite
  index. Network sync is opt-in per account.
- Multi-account, multi-protocol. One account may be IMAP+SMTP, another
  JMAP, another a local Maildir mirrored by mbsync, another Nextcloud
  Mail. All behind the same `EmailSync` trait.
- Performance: 100k+ message mailboxes usable. Search via FTS5. Lazy
  body fetch. Streaming attachments.
- Integrated with the rest of Task: turn an email into a task with one
  keystroke; tasks/projects show their linked emails via `EmailRef`.

## Non-goals (v1)

- Calendaring / contacts (existing CalDAV/CardDAV features cover this).
- PGP/S-MIME (planned later — design must leave room).
- Full HTML/CSS fidelity. Sanitized subset; fall back to text/plain.
- Mail server / MTA. Client only — Stalwart is separate.

## Architecture

Same trio shape as `features/vault/`:

```
features/email/
  email-proto/         #[architect::rpc] trait EmailSync + types
                       Wasm-clean baseline; `vox` feature adds the
                       transport bits. Mirrors vault-proto exactly.
  email-imap/          IMAP backend impl (async-imap + IDLE)
  email-smtp/          SMTP sender, composed by backends that need it
  email-jmap/          JMAP backend (stalwartlabs/jmap-client)
  email-maildir/       Local Maildir backend — first one implemented
  email-nextcloud/     Nextcloud Mail API backend
  email-store/         Maildir + SQLite FTS5 cache; also impls EmailSync
                       so it can sit in front of any of the above
  examples/
    email-client/      Standalone Dioxus harness for dev/test
  plans/
    email-client.md    This doc
```

### The trait

```rust
// features/email/email-proto/src/service.rs
#[architect::rpc]
pub trait EmailSync {
    fn accounts(&self) -> Result<Vec<Account>, EmailSyncError>;
    fn list_folders(&self, account: &str) -> Result<Vec<Folder>, EmailSyncError>;
    fn fetch_envelopes(&self, account: &str, folder: &str, range: SeqRange)
        -> Result<Vec<Envelope>, EmailSyncError>;
    fn fetch_message(&self, account: &str, message_id: &str)
        -> Result<Message, EmailSyncError>;
    fn fetch_attachment(&self, account: &str, message_id: &str, part: &str)
        -> Result<Vec<u8>, EmailSyncError>;
    fn set_flags(&self, account: &str, message_id: &str, delta: FlagDelta)
        -> Result<(), EmailSyncError>;
    fn move_message(&self, account: &str, message_id: &str, dest_folder: &str)
        -> Result<(), EmailSyncError>;
    fn delete_message(&self, account: &str, message_id: &str)
        -> Result<(), EmailSyncError>;
    fn append_draft(&self, account: &str, draft: Draft)
        -> Result<String, EmailSyncError>;
    fn send(&self, account: &str, draft: Draft)
        -> Result<String, EmailSyncError>;
    async fn subscribe(&self, account: String, tx: Tx<EmailEvent>);
}
```

The `#[architect::rpc]` macro derives:
- the sync trait that backends impl directly (in-process is zero-cost);
- the async vox face for remote callers (`EmailSyncClient`);
- the dispatcher mounting points (`serve`, `descriptor`, `layer`).

Backends carry their own state (IMAP pool / JMAP session / Maildir root)
and implement `architect::HasDispatcher` so the bridge knows how to
marshal sync method calls onto the right thread —
`TokioBlockingDispatcher` for network backends, `CurrentThread` for
tests + in-process callers.

## Storage model

- **Maildir** on disk: `~/.local/share/task/mail/<account>/<folder>/{cur,new,tmp}/`
- **SQLite index** alongside (`email-store/schema.rs`):
  - `messages` (envelope + flags + path + content-hash)
  - `messages_fts` (FTS5 over subject + from + to + body_text)
  - `threads` (References/In-Reply-To closure)
  - `pending_ops` (offline write queue)
- **CRDT (Loro)** — later, for cross-device state that isn't on the
  server: user_tags, link relationships (email↔task), read overrides
  for accounts without server-side seen-flag support. **Not** bodies.

Maildir is canonical on disk. Index is disposable, rebuildable by
walking the maildir. Mirrors Task's markdown-first + index-cache
philosophy exactly.

## Sync engine

`email-store` owns the sync loop per account:
1. Open Maildir + SQLite. Reconcile (catch external changes).
2. Subscribe to `backend.subscribe()`. Per event, fetch + write +
   update index + emit local change event.
3. Drafts saved to local maildir + queue table. Send via SMTP/JMAP.
   On failure stays queued.
4. Offline: all reads from disk. Flag changes recorded in
   `pending_ops`, replayed on reconnect.

## UI

Built into `task-ui` (main app). Three-pane:
- Folder list (left) — accounts → folders, unread counts.
- Message list (middle) — virtualized; reads SQLite index.
- Reader pane (right) — sanitized HTML or text/plain; attachment chips.

Dev harness: `features/email/examples/email-client/` mirrors the
editor's `features/editor/examples/playground/` — a standalone Dioxus
binary that wires `email-store` + a chosen backend + the same UI
components, so we can iterate without rebuilding all of task-ui.

## Phases

1. **Bones** — `email-proto` (done at scaffold time), `email-store`
   schema, `email-maildir` (read-only). Example app lists folders +
   messages from a fixture maildir.
2. **IMAP** — `email-imap`: connect, auth, list folders, FETCH
   envelopes, FETCH body, IDLE. Account config loader. Wire to sync
   engine.
3. **Send** — `email-smtp` + draft/compose UI. RFC5322 build via
   `mail-builder` or `lettre::Message`.
4. **JMAP** — `email-jmap` via stalwart's `jmap-client`. Reference impl
   for `subscribe()`.
5. **Index + search** — FTS5 build; search UI; thread reconstruction.
6. **Integration** — wire into `task-ui`. Bridge to `EmailRef`:
   "Link to task" / "Convert to task" actions.
7. **Nextcloud Mail backend** — `email-nextcloud` over the NC Mail HTTP
   API.
8. **Polish** — CRDT for tags/links, attachments UX, signatures, rules,
   PGP scaffolding.

## Open questions

- OAuth (Gmail / M365). pimalaya has `oauth2-lib` — borrow or roll?
- HTML render: `ammonia` for sanitization + Dioxus `dangerous_inner_html`
  in a sandboxed component, or render to MJML-like AST first?
- Attachment storage: in the raw .eml or extracted to
  `attachments/<hash>/`?
- Library-mode (`email-*` reusable outside Task) vs strictly internal?

## Crate picks (late-2025 survey)

- **MIME parse** — `mail-parser` 0.11+ (Stalwart, Apache/MIT). Used by
  `email-maildir` already.
- **MIME build** — `mail-builder` (Stalwart). For drafts. `lettre`'s
  builder is workable but awkward with inline images + threading
  headers.
- **IMAP** — `async-imap` 0.11+ (chatmail) as the main client.
  `imap-codec`/`imap-types` as fallback if we need strict CONDSTORE.
- **SMTP** — `mail-send` (Stalwart) over `lettre`. Either works;
  `mail-send` keeps the stack coherent with `mail-builder` +
  `mail-parser`.
- **JMAP** — `jmap-client` (Stalwart). Exposes EventSource async
  streams.
- **Maildir** — `maildir` 0.6 in `email-maildir`. Considered swapping
  to pimalaya's `maildirs` (MIT, collection wrapper) or `maildirpp`
  (MIT, Maildir++ specific) but deferred — current code already
  implements Maildir++ semantics correctly (root = INBOX, `.Foo`
  siblings, proven by tests). Swap when we need: atomic delivery
  with `fsync(dir)`, flag-manipulation helpers we don't have, or
  notmuch interop.
- **HTML** — `ammonia` + `html2text`.
- **SQLite + FTS5** — `rusqlite` with `["bundled","modern_sqlite"]`.
- **OAuth2** — `oauth2` crate directly.
- **Threading** — JWZ closure in `email-store` (~300 LOC, roll our
  own).

**Surprise:** the Stalwart Labs set
(`mail-parser` + `mail-builder` + `mail-send` + `jmap-client`) is a
coherent matched bundle, all updated within 60 days, all dual
Apache/MIT, all designed to interop. Combined with `async-imap` +
`maildir` + `rusqlite` + `ammonia` + `html2text` + `oauth2`, the whole
client stack is ~10 well-maintained crates.

## What we learned from pimalaya/himalaya

Pimalaya has **two parallel architectures**:

1. **Old: `pimalaya/core/email-lib`** (MIT, async-trait, v0.27). The
   kitchen-sink façade — one `Backend` enum dispatches across
   IMAP/JMAP/Maildir/Notmuch/SMTP/Sendmail. Heavy.
2. **New: `pimalaya/io-*` crates** (Apache-2.0/MIT, sans-io
   coroutines). Himalaya master is migrating onto these. Each
   protocol op is a coroutine with
   `resume(Option<&[u8]>) -> {WantsRead, WantsWrite(Vec<u8>), Ok(T), Err}`,
   runtime-agnostic.

`himalaya` itself (the CLI) is **AGPL-3.0-only** — inspiration only,
no lifting. The `io-*` and `core/*` leaf crates are linkable.

### Decision

**Do not depend on `email-lib`.** Its `Backend` enum collapses our
layered design — pulling it in would subsume `EmailSync` and drag
every backend + tokio + advisory-lock + petgraph along.

**Pull `io-*` leaf crates inside our backend crates** instead:
- `io-imap` inside `email-imap` (Apache-2.0)
- `io-jmap` inside `email-jmap` (Apache-2.0)
- `io-smtp` inside `email-smtp` (Apache-2.0)
- `maildirs` inside `email-maildir` (MIT) — handles cur/new/tmp +
  atomic delivery better than the current `maildir` crate
- `io-discovery` for ISP autoconfig (DNS SRV + Mozilla ISPDB XML +
  Outlook-style)
- `oauth-lib` (MIT) or `io-oauth` (Apache-2.0) for OAuth refresh

The sans-io coroutine shape sits **below** `EmailSync`: our backend
crates own the runtime (tokio for `email-imap`, blocking for
`email-maildir`, wasm-aware for the eventual web target) and feed
the coroutine bytes. Layering preserved.

### Things to steal

1. **Sans-io coroutine driver inside `email-imap`** —
   `{WantsRead, WantsWrite, Ok, Err}` enum. Lets us unit-test the
   IMAP state machine with recorded transcripts, no sockets.
2. **Command-based secret resolution.**
   `Secret { Raw(String), Keyring(String), Command(Vec<String>), EnvVar(String) }`
   — credentials never on disk. `pass`/`bw`/`gnome-keyring` fall out
   for free. Maps to `core/secret`.
3. **Folder aliases as `BTreeMap<Alias, BackendId>`** with
   case-insensitive lookup. Solves Gmail's `[Gmail]/Sent Mail` UX
   permanently and gives the UI stable IDs across renames.
4. **Autoconfig** as a separate `email-autoconfig` crate: DNS SRV +
   Mozilla ISPDB XML + Outlook-style. Onboarding becomes "enter
   email, click next."
5. **Sync as a separate task.** himalaya splits `neverest` (sync) and
   `mirador` (IDLE) out of the CLI. We do the same:
   `email-store` owns watcher tasks + broadcasts deltas on a
   channel, mirroring our `vault` watcher-broadcast wiring (commit
   1fee230). Don't bake IDLE into `email-imap`'s request surface.

### Things to avoid

1. **Monolithic backend enum.** Bakes "which backends exist" into
   the type system. Our `EmailSync` trait + per-crate impl is
   strictly better; don't regress.
2. **`async_trait` on the trait boundary.** Boxes futures, awful on
   wasm. Concrete `impl Future` returns, or sans-io coroutines below
   the trait edge.
3. **CLI-as-source-of-truth config.** himalaya re-reads
   `himalaya.toml` per invocation. We treat config as a typed
   document the UI owns and mutates (OAuth refresh writes a new
   token; add-account wizard appends an entry).

## References

- `features/vault/` — the in-repo pattern we're mirroring.
- pimalaya/core/email — the older, monolithic façade (MIT, read but
  don't link).
- pimalaya/io-imap, io-jmap, io-smtp, io-email — sans-io coroutines
  (Apache-2.0/MIT). Pull these inside our backend crates.
- pimalaya/neverest, pimalaya/mirador — sync / IDLE split out of the
  CLI. Architectural model for our `email-store` watcher.
- pimalaya/himalaya — AGPL-3.0 CLI. Read for UX, don't lift code.
- meli/meli — terminal client (GPL3, ideas only).
- stalwartlabs/{mail-parser,mail-builder,mail-send,jmap-client} —
  matched Apache/MIT bundle, core dependencies.
- async-imap, lettre, ammonia, html2text, rusqlite, oauth2.
