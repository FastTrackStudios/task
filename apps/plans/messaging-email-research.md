# Messaging + Email Research: odysseus & Spectrum (photon.codes)

Date: 2026-07-28. Sources: shallow clone of `github.com/odysseus-dev/odysseus` (dev branch, cloned into this directory), photon.codes docs crawl, Task worktree at `/run/media/Development/herdr-worktrees/FastTrackStudio/worktree-task-consolidation` (read-only).

---

# Part 1 — odysseus

## What it is

`odysseus-dev/odysseus`: a self-hosted AI workspace ("chat, agents, research, documents, email, notes, calendar, local model workflows"). **Python/FastAPI monolith + vanilla-JS static frontend, SQLAlchemy/sqlite, AGPL-3.0.** Created 2026-05-31, **84,103 stars / 145 forks / 993 open issues as of 2026-07-28** — a two-month-old viral project, extremely active, `dev` is the default branch. License caveat: **AGPL-3.0 — ideas are free, code transplants are viral. Do not port code verbatim into FTS; reimplement.**

Repo shape (all paths relative to the clone in this directory, `odysseus/`):

- `app.py` + `routes/*.py` — FastAPI routers per feature (email_routes.py is 6,032 lines; chat, calendar, documents, tasks, skills, shell, webhooks, contacts, companion…)
- `core/database.py` — SQLAlchemy models (users, sessions, EmailAccount, ScheduledTask, Webhook, UserTool…)
- `src/` — the engine: `agent_loop.py` (5,248 lines), `builtin_actions.py` (named non-LLM + LLM actions), `task_scheduler.py`, `event_bus.py`, `integrations.py`, `email_thread_parser.py`
- `mcp_servers/` — **in-process MCP servers**: `email_server.py` (2,920 lines), memory, rag, image-gen
- `companion/` — LAN-client pairing bridge (phone companion)
- Multi-user throughout: every cache table and query is `owner`-scoped (they learned this the hard way — the code carries security-review comments about cross-tenant leaks from unscoped Message-ID caches).

## Agent-integration story (the frame everything hangs on)

Five mechanisms, layered:

1. **In-process MCP servers** (`mcp_servers/email_server.py`) — the assistant's tool surface for email is a proper MCP toolset (~17 tools), not ad-hoc function calls. Tool descriptions encode safety policy in prose: `send_email` says "prefer draft_email so the user can review", `reply_to_email` says "Only use this when the user explicitly says to send now… never invent UID 1". Draft-first is the default posture; immediate send is the exception.
2. **Builtin actions** (`src/builtin_actions.py`) — named, non-conversational actions (e.g. `action_check_email_urgency`) runnable directly by the scheduler without an LLM turn, or as `task_type="action"` scheduled tasks.
3. **ScheduledTask** (`core/database.py:643`) — one table unifies cron and event automation: `task_type` ("llm" prompt vs "action"), `trigger_type` ("schedule" | "event"), `trigger_event` + `trigger_count`/`trigger_counter` (fire every N events), `cron_expression`, `then_task_id` (chaining), `webhook_token` (external trigger), `max_steps` (agent-loop budget), `email_results`, `notifications_enabled`.
4. **Event bus** (`src/event_bus.py`) — `fire_event("email_received", owner)` increments counters on event-triggered tasks and fires those that hit threshold. Events: session_created, message_sent, document_created, memory_added, email_received, etc. Default event-tasks ship with the product ("Tidy Documents every 5 document_created").
5. **Integrations** (`src/integrations.py`) — generic REST integrations where each entry embeds its API cheat-sheet *as prompt text* (ntfy, Home Assistant, Miniflux, linkding), so the agent can drive any of them through one `execute_api_call` tool.

## Email: data model + sync

**The big architectural surprise: there is no local mail store.** Odysseus is **live-IMAP**: every list/read/search hits the IMAP server (with a connection kept warm ≤60s, `routes/email_routes.py:1480`; no IMAP IDLE anywhere). What it persists locally is a side-sqlite of **AI-derived and index caches**, all keyed by **(message_id, owner[, account_id])**:

| table | contents |
|---|---|
| `email_message_index` | envelope index cache: subject/from/to/date_epoch/flags/has_attachments per (owner, account, folder, uid) |
| `email_body_preview_cache` / `email_attachment_metadata_cache` | rendered body payload + attachment listing JSON |
| `email_summaries` | cached LLM summary per message |
| `email_ai_replies` | **pre-generated draft replies** (background pass writes them; the UI click is instant) |
| `email_tags` | LLM triage tags + `spam_verdict`, `spam_reason`, `moved_to` |
| `email_translations` | keyed by body_hash + target_language |
| `email_calendar_extractions` | events extracted from mail → CalDAV, `event_uids` written back |
| `email_urgency_alerts` | urgency verdicts + `alerted` flag |
| `email_boundaries` | **LLM-detected signature/quote start offsets** — computed once, client folds forever without re-calling the LLM |
| `email_event_seen` | first-seen baseline per (owner, account, folder, message_key) — dedupe for the event bus |
| `scheduled_emails` | send-later queue + **agent-draft approval staging** (status machine) |
| `sender_signatures` | learned signature block per from_address |
| `email_away_replies` | vacation-reply ledger (cooldown/period keys) |

Repeated hard-won lesson in comments: **Message-IDs are global** (a newsletter shares one Message-ID across all recipients), so every AI cache had to become (message_id, owner, account_id)-scoped after a cross-tenant leak (their review "C2").

**Accounts** (`core/database.py:386` `EmailAccount`): multi-account per owner, one `is_default`; IMAP host/port/user/password + SMTP host/port/security(ssl|starttls|none); passwords Fernet-encrypted at rest (key file mode 0600; threat model = "stolen sqlite backup"). **Auth: plain IMAP/SMTP password or Google OAuth via SASL XOAUTH2** (`routes/email_helpers.py:53-176` — token refresh against oauth2.googleapis.com, then `conn.authenticate("XOAUTH2", …)`); i.e. Gmail over ordinary IMAP, **not** the Gmail REST API. **No Microsoft Graph/Outlook OAuth** (`docs/email-outlook.md` says so explicitly — Outlook basic auth is dead, so O365 simply doesn't work).

**Sync/processing engine** (`routes/email_pollers.py`): a single `_auto_summarize_pass` poller (60s when auto-reply enabled, else 30min) fans out over all enabled accounts, IMAP-SEARCHes `SINCE <days_back>` in INBOX (+Sent if calendar extraction is on), takes the newest ~30 UIDs, skips anything already in the caches, and runs per-message LLM work under a **per-pass budget** (`max_process` default 3–5 messages/pass — cost control, keeps a pass under their 5-minute action budget). Feature flags per account: auto_summarize, auto_reply (draft-only vs away-mode), auto_tag, auto_spam (classify + move to detected Junk folder), auto_calendar.

**Threading**: no real conversation model (no References-based thread index). Instead `src/email_thread_parser.py` — a server-side port of talon/email-reply-parser that splits ONE message body into a tree of quoted reply turns (multilingual "On <date> X wrote:" attributions in 20+ locales, Outlook header blocks, blockquote/`> ` nesting), returned as `[{level, body_html, meta}]` with a **parser-version-stamped cache** (`THREAD_PARSER_VERSION = 6`; version bump invalidates). Good for rendering, not a substitute for real threading.

## Email × AI: the genuinely good parts

1. **Background pre-generation + cache-as-contract.** Summaries, draft replies, tags, urgency, translations, fold boundaries are all computed *once* in the background pass and stored owner-scoped by Message-ID. The UI and the MCP tools read the same caches (`list_emails` returns "cached AI summary for each"). LLM cost is bounded (N messages/pass), user-perceived latency is zero.
2. **Human-in-the-loop send approval.** Agent-written mail is staged into `scheduled_emails` in an awaiting-approval status; `GET /email/pending` lists staged drafts; `POST /email/pending/{sid}/approve` flips status → `pending` with `send_at=now`, and the 30s scheduled-send poller delivers (`routes/email_routes.py:4287-4330`). The agent can *never* directly SMTP; approval and delivery share the send-later machinery.
3. **Draft-first MCP tool design.** Separate `draft_email` / `draft_email_reply` / `ai_draft_email_reply` (create reviewable compose documents, threaded with In-Reply-To/References) vs `send_email` / `reply_to_email` (explicit-consent immediate send). The safety policy lives in the tool descriptions where the model actually reads it.
4. **Urgency scoring with alert-once state** (`src/builtin_actions.py:1847` `action_check_email_urgency`): heuristic pre-pass (regex for "locked out", "waiting outside"…) then LLM JSON verdict `{score:0-3, tags:[], spam:bool, reason}` with a small allowed-tag taxonomy (urgent, reply-soon, action-needed, calendar, bills, receipt, travel). State keeps `notified_uids`; a reminder fires **only when a previously-unnotified UID scores ≥2**. Also guards against classifying its own outbound mail as urgent (self-address check hoisted out of the loop).
5. **First-sync baselining** (`routes/email_routes.py:280-360` `_record_email_received_events`): the first scan of a folder only *baselines* `email_event_seen` (fires nothing); after that, `email_received` events fire only for genuinely new message keys, capped at 50/pass. No notification storm on account add. This is exactly the pattern a Task notification hub needs.
6. **Reply context retrieval with an explicit prompt-injection blast-radius analysis** (`routes/email_helpers.py:1554` `_fetch_sender_thread_context`, `:1665` `_pre_retrieve_context`): AI replies get (a) the last N mails from the same sender across INBOX/Sent/Archive/Drafts incl. attachment text, and (b) keyword-retrieved past-mail/contacts context — but retrieval (b) is **gated on known senders** (must be in contacts or have prior mail), terms ≥5 chars, multiword-only for unknown senders, ≤3 terms, precisely because a crafted inbound mail could otherwise exfiltrate private context into an auto-reply. The security reasoning is written in the docstring.
7. **Learned writing style + per-sender signatures**: `POST /email/extract-style` learns the user's voice from sent mail (global + per-account styles, fed into every draft prompt); `sender_signatures` caches each correspondent's signature block for cleaner quoting/folding.
8. **Unsubscribe workflow** (`routes/email_routes.py:487-2630`): parse RFC `List-Unsubscribe` headers into *reviewable* candidate actions, dedupe by sender, execute only mailto-based unsubscribes (no arbitrary GETs), review-before-execute.
9. **Away/vacation replies done carefully** (`email_pollers.py`): automated-sender detection (skip no-reply/bulk), per-period keys + cooldowns so one sender gets one reply per period, only for mail arriving after away-mode was enabled.
10. **LLM fold boundaries** (`email_boundaries`): one LLM call finds sig/quote char offsets per message; client folds quoted text forever after from the cache. Cheap trick, big UX win.

## Email: the ordinary/weak parts (don't copy)

- Live-IMAP with no local store: their own ROADMAP admits "fetching, searching, opening, deleting, sending can feel slow". Task's maildir/store/sync design is already strictly better here.
- No IMAP IDLE, no JMAP, no push — everything is polling (60s/30min).
- A pile of ad-hoc sqlite side-tables with hand-rolled rebuild-copy-swap migrations in route files.
- Threading = quoted-text parsing per message, not a conversation graph.
- 6,000-line route files; agent loop is a 297KB Python file.

## Notifications / eventing

Three tiers, all simple:
- **In-app**: `task_scheduler.add_notification` → in-memory owner-tagged queue (cap 50) → client **polls** `/api/tasks/notifications` and drains via `pop_notifications(owner)` (strict owner scope after a leak fix). `body` (truncated 500 chars) enables rich browser Notifications; per-task `notifications_enabled` and `output_target="notification"`.
- **Phone push**: **ntfy** as a first-class integration (topic-based push; connectivity probe sends a test message). Cheap, self-hostable, no APNs pain.
- **Outgoing webhooks** (`core/database.py:561`): HMAC-SHA256-signed, comma-separated event-type subscriptions, last_status/last_error bookkeeping.
Plus the internal event bus (above) which turns events into agent-task triggers — the most interesting part: **notifications and automation share one event stream**.

## Messaging (iMessage/SMS/WhatsApp)

**Nothing.** No iMessage, SMS, WhatsApp, Telegram, or Matrix anywhere in the tree (grep confirms; only false positives). The `companion/` dir is a LAN pairing bridge (one-time-token minting for phone clients), not messaging. The `swift/` dir is an MLX image bridge.

## Mapping onto Task (features/task/email-*, agent-*)

*(See Part 3 for the Task-side inventory this maps onto.)*

Where each good idea lands in Task's structure:

1. **AI-derivation cache** → a new `email-ai` slice (or a table family in `email-store`): `derived(message_id, owner, account_id, kind, payload, model, version, created_at)` with kind ∈ {summary, draft_reply, tags, urgency, boundaries, translation}. Computed by a background pass in the `email-sync` engine after new-message ingest; served through email-proto so both email-ui and agent tools read the same rows. Task has a real local store, so unlike odysseus the pass reads locally — no IMAP round-trips.
2. **Send-approval staging** → an `outbox` table in email-store with a status machine (`draft_agent` → `approved/pending` → `sent|failed`), a small `EmailOutbox` RPC (list_pending/approve/reject/schedule), and a `#[subscribe]` stream so the UI badge updates live. Agent email tools get *only* stage-to-outbox; SMTP submission lives behind the approval flip. This is the single highest-value transplant.
3. **Event baselining + alert-once** → in the email-sync engine: on first folder sync, record baseline and emit nothing; afterwards emit `EmailEvent::Received` into an architect PubSub hub; a notifications slice subscribes and applies alert-once state (notified set per account). Exactly matches the #[subscribe]/PubSub idiom already in the tree.
4. **Draft-first agent tool surface** → Task's MCP server (the existing 15-tool surface) grows email tools mirroring odysseus's split: `email_list/read/search` (read), `email_draft/draft_reply` (stage to outbox), and *no* direct send tool at all in v1.
5. **Urgency/triage taxonomy** → keep their tag set + 0–3 score as the initial schema for `kind=tags/urgency`; heuristic pre-pass before any LLM call; guard against self-sent mail.
6. **Injection-gated reply context** → when the agent drafts a reply, context retrieval gated on known-contact senders (Task has real contact entities in the vault: Records/contacts), with term-length/multiword caps. Port the *policy*, reimplement the code.
7. **ntfy tier** → the cheapest possible push for the notifications slice until native push exists; topic per user/org.
8. **LLM fold boundaries + versioned parse cache** → email-ui reader: store sig/quote offsets per message with a parser/model version stamp.
9. **Unified automation trigger** → ScheduledTask's "one table: cron OR event(count) OR webhook-token, optional then-chain" is a nice minimal shape for Task Routines to grow toward.

---

# Part 2 — Spectrum (photon.codes)

## What spectrum-ts actually is (suspicion corrected)

The suspicion ("a TypeScript library for reading/sending iMessage from macOS") is **mostly wrong as the headline, right as a footnote**. spectrum-ts (`github.com/photon-hq/spectrum-ts`, **MIT**, ~1,200 stars, TypeScript) is a **multi-platform messaging SDK for deploying AI agents** — iMessage, WhatsApp Business, Telegram, Slack, Terminal, and voice-over-SIP behind one API, custom platforms via `definePlatform`. Photon (photon.codes) is the company; launched publicly April 2026; near-daily releases (v12.5.0 published 2026-07-28).

Two very different iMessage paths:

- **Cloud (`@spectrum-ts/imessage`)** — Photon-hosted iMessage lines over gRPC. No macOS in your stack at all. Full feature set: tapbacks, replies, edits, unsend, effects, inbound read receipts, typing indicators, streamed text, attachments with lazy `getAttachment(guid)`, contact cards, group management (Business tier), SMS/RCS fallback, iOS 26 extras (polls, mini-app cards, voice messages, location). Pricing: Free $0 (shared numbers, 10 allowlisted users) / Pro $25/mo (100 users) / **Business $250/line/mo** (dedicated lines, groups) / Enterprise. Quotas: 5,000 msgs/server/day, 50 new conversations/line/day.
- **Local (`@spectrum-ts/imessage-local`)**, built on **`@photon-ai/imessage-kit`** (npm, MIT, v3.0.0 Apr 2026) — exactly the suspected thing: reads `~/Library/Messages/chat.db` on a Mac signed into Messages.app, sends via `osascript` AppleScript driving Messages.app. Key technical facts:
  - **WAL-based watching of chat.db** (event-driven on sqlite WAL changes, not naive interval polling); one active watcher per SDK instance; no daemon — lives in your Node/Bun process.
  - **`attributedBody` handled**: Ventura+ NSAttributedString blobs decoded via `@parseaple/typedstream` — the classic macOS-13+ "empty text column" problem is solved.
  - Handles the **macOS 26 chat-GUID format change** (`iMessage;+;chat…` → `any;+;chat…`) — good sign for macOS-27 survivability, though macOS 27 beta itself is unverified by anyone.
  - Requires: macOS signed into Messages, **Full Disk Access** for the host process, Node ≥20 (needs `better-sqlite3`) or Bun (built-in sqlite).
  - Local-mode gaps: **no tapbacks/replies/edits/unsend/read-receipts/typing/group management** (effects throw `UnsupportedError`); send confirms osascript exit, **not delivery**; SMS relay = whatever Messages.app relays (undocumented as a feature).

## API shape

Library, not a server (webhook adapters for Hono/Express/Elysia exist; a separate management REST API at spectrum.photon.codes powers their dashboard). Primitives: **Message, Space (conversation), User, Provider**. Receive = async-iterable stream (`for await (const [space, message] of app.messages)`) or webhook mode (HMAC-SHA256, 5-min replay window). Send = promise-based `space.send()` / `message.reply()` / `space.responding(fn)` typing wrapper. A lower-level sibling `@photon-ai/advanced-imessage` gives direct HTTP/gRPC to their cloud gateway without the agent framework.

## Verification honesty

Sub-agent fetched: photon.codes home + pricing; docs pages introduction, getting-started, providers/imessage (+connection-and-routing, +messaging-features), webhooks, troubleshooting/imessage, content/contacts, api-reference/introduction, /docs/llms.txt (nav lists ~70 pages; only the above were fetched); GitHub repos + releases; registry.npmjs.org metadata (www.npmjs.com 403'd; registry JSON used instead). Unverified: WhatsApp/Telegram/Slack/voice provider subpages, best-practices, CLI pages. One correction made: GitHub releases UI suggested 2024 dates; npm timestamps prove 2026. Docs mention no Sonoma/Sequoia/SIP breakage; **nobody has verified macOS 27 beta** — that risk is ours.

## Integration design: the airlock messaging bridge

Goal: text people from Task, and ingest iMessage/SMS conversations related to Task contacts/projects. The user owns a headless Mac mini ("airlock") already running as a CI/build machine on the tailnet — but it runs **macOS 27 beta**, and chat.db schema/AppleScript automation on a beta OS is the single biggest risk (imessage-kit handled the macOS-26 GUID change quickly, which is encouraging but not proof).

**Recommended architecture — self-hosted, no Photon cloud:**

```
Messages.app + chat.db (airlock, FDA granted)
        │  imessage-kit (WAL watcher + AppleScript send)
        ▼
fts-messages-bridge  (small Bun/Node service on airlock, ~200 lines)
        │  HTTP + WS on tailnet :4047, static bearer token
        ▼
task-server messaging slice (Rust)                      Task UI
  messaging-proto / -store / -bridge-client  ──────►  messaging panel
```

1. **Bridge service (airlock, TypeScript, uses imessage-kit directly — skip the Spectrum agent framework)**: endpoints
   - `GET /v1/messages?since_rowid=N&limit=…` — incremental pull straight off chat.db ROWIDs (chat.db's `message.ROWID` is the natural monotonic cursor; this makes the bridge stateless and the *server* own the cursor).
   - `WS /v1/stream` (or long-poll fallback) — push of new messages from the WAL watcher, so latency is seconds, with the since_rowid pull as the catch-up/repair path after bridge or server restarts.
   - `POST /v1/send {to | chat_guid, text, attachments?}` — AppleScript send; returns accepted (not delivered — be honest in the UI: "handed to Messages").
   - `GET /v1/chats`, `GET /v1/attachment/{guid}` (file streamed from `~/Library/Messages/Attachments`).
   - Health: `GET /v1/ping` reporting Messages-signed-in + FDA status.
2. **Auth**: the bridge listens on the tailnet only; static bearer token in the same spirit as `TASK_WATCH_TOKEN` (env `FTS_MESSAGES_TOKEN` on both ends). This mirrors the existing **watch_bridge.rs HTTP-bridge convention** for non-vox devices — the Mac bridge is exactly such a device (no Rust/vox on the airlock service; plain HTTP+WS).
3. **Sync model**: hybrid — WS push for freshness, `since_rowid` polling (e.g. every 60s) as the invariant-bearing mechanism. Server stores `last_rowid` per bridge. Tombstones aren't needed v1 (Messages rarely deletes); re-pull window on restart covers edits (macOS edits create new rows referencing the original).
4. **Task side — a `messaging` plugin mirroring the email-* slice shape**:
   - `messaging-proto`: `Conversation`, `Message` (id = chat.db guid, handle, direction, text, attachments, sent_at, service = iMessage|SMS), `MessagingBridge` RPC trait + `#[subscribe] fn message_events()` stream.
   - `messaging-store`: local persistence, contact linking by normalized handle (phone/email) against vault `Records/contacts` — same identity resolution email uses for addresses.
   - `messaging-bridge` (server): the HTTP/WS client to airlock, cursor management, PubSub hub publishing `MessageEvent::Received` → notification slice reuses the email alert path (with odysseus-style first-sync baselining so importing years of chat history fires zero notifications).
   - `messaging-ui`: conversation list + thread view + composer; send button → server → bridge `POST /v1/send`; optimistic "sending → handed-off" states.
5. **Agent story (later)**: agent tools `messages_list/search` (read) and `message_draft` (stage-for-approval, reusing the same outbox-approval machinery as email — never direct send). This is where odysseus's approval pattern and Spectrum's agent framing converge.

**Risks, ranked**: (1) macOS 27 beta — chat.db schema and osascript automation unverified there; mitigation: the bridge is ~200 lines over imessage-kit, testable in an hour on airlock; if 27 breaks it, options are a macOS 26 VM/partition or waiting out the beta. (2) Full Disk Access + Automation permission grants on a headless box — must be done once via screen sharing; they survive reboots but can be reset by OS updates. (3) Apple ToS/gray-zone: AppleScript send is the same mechanism every OSS iMessage bridge uses (mautrix-imessage, BlueBubbles); personal-use scale, low risk, but never expose the bridge off the tailnet. (4) Send-rate: pace sends (imessage-kit already paces multi-file sends ~500ms). (5) Attachments disk growth on the server — store proxies/thumbnails, fetch originals lazily from the bridge.

**Why not Photon cloud**: $250/line/mo for dedicated-line features, messages transit a third party (personal conversations!), and the user already owns the Mac. Cloud is the fallback only if macOS 27 permanently breaks local mode. **Why imessage-kit rather than all of spectrum-ts**: the agent-framework layer (Spaces/providers/gRPC auth) is Photon-cloud-shaped; local mode only needs the kit's watcher + sender. WhatsApp via Spectrum requires WhatsApp *Business* cloud API — not the user's personal WhatsApp — so it's out of scope for v1.

---

# Part 3 — Task-side inventory (fit check)

From `/run/media/Development/herdr-worktrees/FastTrackStudio/worktree-task-consolidation` (read-only). Key facts that shape what to build:

## Email slices (features/task/email/, 13 crates)

- **email-proto** (`features/task/email/email-proto/src/service.rs`): `#[architect::rpc] trait EmailSync` — accounts, list_folders, fetch_envelopes(SeqRange), fetch_message, fetch_attachment, set_flags(FlagDelta), move_message, delete_message, append_draft, **send(Draft)**, plus `#[subscribe] fn changes() -> EmailChange` (`EmailEvent::{NewMessage, FlagsChanged, Moved, Deleted, FolderListChanged, Resync}`; changes-only contract, client-side account filtering). Model: `Account/Folder(+FolderRole)/Envelope(thread_id, snippet, unix-ms dates)/Message(bodies inline, attachments lazy by part)/Draft/FlagDelta`. No `Thread` proto type.
- **Backends**: `email-maildir` (reads only; **the only backend the server mounts** — `apps/task/server/src/lib.rs:863,2331-2339`), `email-imap` (full read+write **and real SMTP send** via email-smtp, 818 lines — but never constructed by the server, so send is currently dead code), `email-jmap` (accounts+folders only), `email-smtp` (build_message wasm-clean + SmtpSender native), `email-autoconfig` (ISPDB/SRV discovery), `email-secret` (keyring/env/command secrets).
- **email-store**: maildir source-of-truth + disposable rusqlite index — FTS5 (unicode61, diacritics), **JWZ threading**, `pending_ops` offline replay queue (**unused so far**), rebuild_from_disk.
- **email-sync**: `SyncEngine` poll-diff loop per account (spawn_blocking over the sync trait, snapshot diff → EmailEvents, persists to store before diffing; `SyncEvent` superset gives the UI cycle/spinner state). Gaps: backend `changes()` not forwarded; **no outbox drain**.
- **email-ui**: **reader only** — account chips + INBOX envelope list; no compose/reply/flags; doesn't subscribe to the stream. Routed at `crates/task/ui/src/routes.rs:182`.
- **email-link**: Message-ID ↔ entity links, frontmatter-canonical (`[[email://<message-id>|Subject]]`), with `EntityKind::person()` — **the email↔contact seam, currently unwired**.

## Agents, contacts, realtime, bridge

- **Agent tool surface** = the MCP server (`apps/task/server/src/mcp.rs`, `POST /org/{slug}/mcp`, 15 tools, none email). Tool bodies call in-process `OrgAppState` backends directly — `org.email` is already on the state, so email tools are a pure-addition. `agent-proto` has an **Approvals** capability trait (list_pending_approvals/resolve_approval) whose approval-kind docs already anticipate "send a message externally (email, Slack, …)".
- **Contacts** (`features/task/contacts/`): vault-canonical `Records/contacts/<id>.md`, `Contact` entity with newline-joined `emails`/`phones` (+ `primary_email()`), CardDAV pull, `#[subscribe]` events, mounted with a stream layer. **No contacts↔email linkage exists today**; no contacts-ui crate.
- **Realtime**: no notification slice exists. The convention is uniform: `#[subscribe]` method on the rpc trait → backend holds `architect::PubSub::sliding(N)` and impls `*StreamSource::*_hub()` → server `.merge(stream_layer(...))`. ~16 streams already mounted (email's at `lib.rs:2339`). The agent router shows the multi-backend pattern: **one shared hub injected into both backends at construction**.
- **watch_bridge.rs** (`apps/task/server/src/watch_bridge.rs`, 233 lines): the canonical non-vox HTTP device bridge — plain axum JSON under `/org/{slug}/watch/v1/*`, `Authorization: Bearer` dual-accept (static `TASK_WATCH_TOKEN` → deterministic local-owner UUID, or a real architect session token), handlers call the same in-process backends as the vox layer, hand-written DTOs to decouple device wire from proto churn. mcp.rs explicitly adopts the same token rule (`TASK_MCP_TOKEN`). **The messaging bridge client should follow this file's conventions in reverse** (Task as HTTP client instead of server, same token discipline).

## Fit verdict

Task's email foundation is architecturally *ahead* of odysseus (real local store, FTS5, JWZ threads, typed rpc + streams, backend abstraction incl. JMAP stub) but *behind* in product surface (no compose/send path wired, no AI layer, no triage, no notifications, reader-only UI). Odysseus is exactly the map of "what to build on top", and its mistakes (live-IMAP, ad-hoc caches) are ones Task's skeleton already avoids.

---

# Part 4 — Verdict & recommended v1 scope

## Ranked transplantable ideas

1. **Outbox with human-in-the-loop agent approval** (odysseus `scheduled_emails` + `/pending/approve`; converges with agent-proto's existing Approvals trait). One status-machine table serves send-later, agent-staged drafts, and offline send replay (email-store's unused `pending_ops` is the natural home). Agents can only stage; approval flips status; a sync-engine drain delivers via SMTP.
2. **Owner-scoped AI-derivation cache keyed by (message_id, account, kind, version)** — summaries, draft replies, tags, urgency, fold boundaries computed once in a bounded background pass (N msgs/pass), read identically by UI and agent tools. Odysseus proves the UX (instant summaries/replies) and documents the cross-tenant Message-ID pitfall to avoid.
3. **First-sync baselining + alert-once notification state** (`email_event_seen`, `notified_uids`): baseline silently on first sync, notify only never-before-notified messages, cap per pass. Prereq for any notification slice; applies identically to the iMessage bridge (importing chat history must fire zero notifications).
4. **Draft-first agent tool split**: read tools + draft/stage tools, *no* direct-send tool in v1; safety policy written into tool descriptions (odysseus's "prefer draft_email…", "never invent UID 1"). Maps directly onto mcp.rs's additive tool catalog + server_instructions.
5. **Triage taxonomy + 0–3 urgency score with heuristic pre-pass** (urgent, reply-soon, action-needed, calendar, bills, receipt, travel; spam verdict + move; self-mail guard). A proven, small schema to start `kind=tags` with.
6. **Injection-gated reply context**: reply drafting retrieves sender history/contact context only for known contacts, with term-length/multiword/count caps — the policy is the transplant, and Task's real contacts entity makes the "known sender" check better than odysseus's.
7. **The messaging bridge architecture itself** (Spectrum-derived): imessage-kit's WAL-watch + AppleScript-send on airlock behind a ~200-line HTTP/WS service; Task consumes it as a `messaging` slice shaped like email-* (proto/store/bridge/ui) under watch_bridge token discipline.
8. **LLM fold boundaries + versioned parser cache** (sig/quote offsets computed once; `THREAD_PARSER_VERSION`-stamped cache invalidation) for the email-ui reader.
9. **ntfy as the v1 push tier** for a notification slice (self-hostable topic push; connectivity probe sends a test message), until native push exists.
10. **Unsubscribe review workflow** (List-Unsubscribe parse → deduped reviewable candidates → mailto-only execute) and **learned writing style / per-sender signatures** — nice-to-haves once drafting exists.

## Spectrum verdict

**Feasible, and the local path is free (MIT) and well-engineered** — `@photon-ai/imessage-kit` solves the two classically hard parts (attributedBody decoding via typedstream; WAL-event watching instead of polling) and already absorbed one macOS GUID format change. Use the kit, not the Photon cloud (cost, privacy, third-party transit) and not the full Spectrum agent framework (cloud-shaped). Risks: **macOS 27 beta on airlock is the big unverified one** (an hour's smoke test on airlock is the first action; fallback = macOS 26 VM or wait); one-time GUI session needed to grant Full Disk Access + Automation; local mode lacks tapbacks/read-receipts/typing/groups-management and confirms handoff, not delivery; AppleScript send is ToS-gray like every OSS bridge — keep it tailnet-only, personal-scale. WhatsApp is out (Spectrum's is Business-API cloud only).

## Recommended v1 scope

**(a) Email expansion** — order of operations:
1. Wire send: construct SMTP-capable backends in the server (imap backend or maildir+smtp composite), add the outbox table + drain in email-sync, compose/reply UI in email-ui.
2. Approval flow: agent-staged outbox status + Approvals integration + `#[subscribe]` outbox events.
3. MCP email tools (list/read/search/draft/draft_reply — no send).
4. Derivation cache + bounded background triage pass (summary, tags/urgency first; draft replies second).
5. Notification baseline/alert-once state + ntfy delivery; email↔contacts resolution via email-link `person()` + `Contact::primary_email()`.

**(b) Mac-mini messaging bridge v1**: smoke-test imessage-kit on airlock (macOS 27) first; then the bridge service (ping/messages-since-rowid/stream/send/chats/attachment, bearer token, tailnet-only), then `messaging-proto` + server bridge client with store + contact linking + PubSub stream, then a minimal conversation UI with send. Explicitly defer: tapbacks, group management, WhatsApp, agent send (add read+draft agent tools only after the email approval flow exists).

## Unverified / honesty

- Spectrum: ~60 of ~70 doc pages unfetched (provider subpages for WhatsApp/Telegram/Slack/voice, best-practices, CLI); www.npmjs.com blocked (registry JSON used); no first-hand test of imessage-kit anywhere, and **zero evidence about macOS 27 beta compatibility either way**.
- Odysseus: explored the `dev` branch clone at depth 1; conclusions are from code reading, not running it. Star/fork/issue counts from the GitHub API 2026-07-28. AGPL-3.0 — reimplement ideas, never port code.
- Task worktree facts are from the consolidation worktree, which may drift from main.
