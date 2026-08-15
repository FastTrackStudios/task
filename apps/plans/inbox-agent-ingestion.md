# Inbox agent ingestion — email / text → the single trusted system

**Status:** partially shipped — needs triage (2026-07-27). `features/task/agent/agent-inbox` (parsers / bridge / prompts / templates) exists; the watcher + end-to-end loop was not verified.

**Goal:** let an AI agent watch external streams (email first, texts later) and
drop *noteworthy* items into the inbox as `InboxItem`s, so they flow through the
same capture → process → write loop as everything else. The inbox stays the one
trusted system; the agent is just another **producer**.

## Why this fits cleanly

The inbox is already a sink anything can write to:
- `Inbox::upsert_inbox_item` (vox) / `task inbox add` (CLI).
- `InboxItem.source` is free-form → `"email"` / `"sms"`.
- `InboxItem.body` is markdown → a link back to the original lives in the body.
- The process/review UI already shows a `via {source}` badge and snoozes/promotes
  agent-sourced items exactly like manual captures.

So no inbox changes are required for v1. The work is the **producer** + the
**triage decision**.

## What already exists (grounded)

- **Email access** — `features/email/email-proto::EmailSync` (`fetch_envelopes`,
  `fetch_message`, `subscribe`, flags/move/send). Backends: `email-imap` (working),
  `email-jmap`, `email-maildir`, `email-nextcloud`. `email-sync::SyncEngine` already
  has a configurable **poll loop** (`run_cycle()`, default 60s) broadcasting new-message
  events. No Gmail OAuth yet — accounts are plain IMAP/JMAP/Maildir (Gmail works today
  via an IMAP app-password).
- **Agent runtime** — `features/agent` has a task queue (`AgentTaskQueue`: claim /
  status / complete, dep-checked, SQLite per queue) and `agent-dispatch::schedule_recurring`
  (idempotent-per-day task dispatch). Turn dispatch (`TurnDispatch`) runs LLM turns.
- **Missing** — (a) no background daemon loop in `apps/server` (would add one, or run
  via CLI+cron); (b) no SMS/text source at all (needs an external bridge — Twilio inbound,
  KDE-Connect, or an Apple Messages/Android bridge); (c) no Gmail OAuth (IMAP app-password
  sidesteps it).

## The two layers

1. **Ingestion** — fetch new messages for an account (reuse `email-sync`).
2. **Triage agent** — for each new message decide: *is this something to remember or
   act on?* If yes, write a concise `InboxItem` (one-line summary + a markdown link back
   to the message, `source="email"`); if no, skip. This decision is the whole point —
   without it the inbox floods. Runs as an LLM turn / agent task with a tight rubric
   (e.g. "needs a reply, a commitment, a date, or info worth keeping" → capture; receipts,
   newsletters, automated noise → skip).

## Capture shape (proposed)

```
source: "email"
kind:   "fleeting"
body: |
  Reply to Sarah re: Q3 invoice — she's waiting on the revised figure.
  [open email](message://<account>/<mailbox>/<uid>)   ← deep link back
created: <rfc3339>
```
The deep link can be a `message://` URI the email UI resolves, or just the
subject + sender if no resolver exists yet. The link-back keeps the temporal
contract: during triage you can jump to the source.

## Slices

- **Slice 1 — email → inbox (CLI, manual cadence).** `task inbox ingest-email
  --account <id> [--since <when>] [--dry-run]`: run one `email-sync` cycle, run the
  triage rubric over new envelopes (LLM or, for a first cut, a deterministic
  heuristic), and `upsert_inbox_item` the keepers. Driven by an external cron /
  systemd timer. ~200–300 lines, no server changes. **Recommended start.**
- **Slice 2 — server daemon.** Spawn the `SyncEngine` per configured account in
  `OrgAppState`, bridge new messages → triage → inbox automatically. No external cron.
- **Slice 3 — texts/SMS.** Pick a source (Twilio inbound webhook is the cleanest),
  same triage → inbox path with `source="sms"`.
- **Slice 4 — richer triage + dedup.** Thread-aware (one item per thread), don't
  re-capture already-seen messages, learn from what you snooze/delete.

## Open decisions (need input)

- **Which mailbox first** — a generic IMAP account (incl. Gmail via app-password) vs
  building Gmail OAuth.
- **How aggressive** — auto-capture keepers, or stage them for a one-tap "accept into
  inbox" review first (less trust required up front).
- **Triage brains** — LLM turn (uses the agent backend) vs a simple rule heuristic for v1.
- **Texts** — which SMS source, or defer.

## Related

- [[project_inbox_feature]] — the inbox trio + process flow this feeds.
- `features/email/plans/email-client.md` — the broader email client scope.
