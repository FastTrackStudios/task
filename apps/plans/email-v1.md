# Email v1 — product layer status

Scope source: `apps/task/plans/messaging-email-research.md` (Part 4,
"Recommended v1 scope (a)"). The skeleton (maildir store, FTS5, JWZ
threading, typed EmailSync + `#[subscribe]` stream, working SMTP crate)
predates this branch; this doc tracks the product layer built on top.
Ideas from odysseus are clean-room reimplementations — its AGPL code was
never opened, only the research doc's descriptions.

## Status

| # | Item | State |
|---|------|-------|
| 1 | Wire send (SMTP through the mounted maildir backend) | DONE |
| 2 | Outbox + approval state machine + poller | DONE |
| 3 | Compose UI (reply/new) + outbox panel | DONE |
| 4 | Triage pass (derivation cache, heuristics only) | DONE |
| 5 | Baseline + alert-once notification surface | DONE |

Gates green as of 2026-07-28: workspace check (`--exclude
vox-discover`), `task-app-web` on wasm32 (after `just css`),
`task-server` tests, `ui --lib`, all email slice tests incl. the
outbox round-trip e2e (`email-product/tests/outbox_roundtrip.rs`)
and the baseline/alert-once e2e (`tests/notify_baseline.rs`).

To make a real account sendable: drop an `account.json`
(`email_config::AccountConfig`, JSON) into
`<org>/vault/Mail/<account>/` with `backend.kind = "maildir"` and
a `submit` block (host/port/tls/username/password secret) — the
server's account discovery wires the SMTP submitter from it.

## Design decisions

- **Send path**: `email-maildir::Backend` grows a per-account
  `Submit` transport (implemented by `email_smtp::SmtpSender`, mockable
  in tests). `send` = build RFC5322 bytes → submit → write the Sent
  copy into the account maildir (`.Sent/cur`, `\Seen`) → publish
  `EmailEvent::NewMessage { folder: "Sent" }` on the changes stream.
  Account SMTP config comes from `email-config::AccountConfig`
  (`BackendKind::Maildir` gains an optional `submit: SmtpConfig`),
  loaded from `<mail_root>/<account>/account.json` by the server's
  account discovery.
- **Outbox**: state machine lives in `email-store` (new `outbox` table;
  the dead `pending_ops` table is dropped). States:
  `Draft → PendingApproval → Approved → Sending → Sent | Failed(reason,
  retries)` (+ `Cancelled`). Proto surface is a second rpc trait in
  `email-proto` (`EmailProduct`: `list_outbox` / `submit_draft` /
  `approve` / `cancel`), served by a new `email-product` backend crate.
  A server-side poller delivers Approved entries via `EmailSync::send`
  with exponential backoff; every transition publishes
  `EmailEvent::OutboxChanged` on the *same* EmailChange hub the maildir
  backend serves (hub shared at construction, publish-after-write).
  Agents draft; only user approval releases delivery.
- **Derivations**: `derivations(message_id, kind, version, payload)`
  table per account store; kinds v1 = `urgency` (0–3) + `tags`
  (action-needed, waiting, newsletter, receipt, calendar, social,
  other). Heuristics only (headers, List-Unsubscribe, contacts lookup,
  self-mail guard); `DerivationEngine::derive_llm` is a default-
  unimplemented trait hook for the agent plugin. Bounded pass: N=5
  messages/tick.
- **Alert-once**: first sight of an account baselines silently
  (`notify_state` rows marked notified); after that new messages get
  exactly one unnotified mark, capped per pass. Surface for the (other
  agent's) notifications system: `EmailProduct::unnotified(account,
  limit)` + `mark_notified(account, ids)`. We do NOT wire into
  features/task/notify — that team consumes this proto surface.

## Handshake for the notifications agent

- `EmailProductClient::unnotified(account, limit) -> Vec<message_id>`
- `EmailProductClient::mark_notified(account, ids) -> u32`
- New-message signal: subscribe to the existing EmailChange stream
  (`EmailEvent::NewMessage`), then drain `unnotified` + `mark_notified`.

## Deliberately deferred (from the research doc)

- MCP email tools (draft-first agent tool split) — mcp.rs is owned by
  another agent this cycle.
- LLM derivations (summaries, draft replies, fold boundaries) — the
  `DerivationEngine` hook exists, no engine wired.
- Spam verdict + auto-move, unsubscribe workflow, learned writing
  style, away replies, ntfy push, injection-gated reply context.
- IMAP-backed product path (the server still mounts maildir only).
- Messaging bridge (research Part 2) — separate effort.
