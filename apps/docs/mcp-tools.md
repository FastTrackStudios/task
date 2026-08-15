# MCP tool catalog

Task exposes itself to agents over MCP at `POST /org/{slug}/mcp`
(JSON-RPC 2.0, Streamable HTTP — every method answers inline, no SSE
upgrade). The server side lives in `apps/task/server/src/mcp.rs`; this
page is for **agent authors** wiring a client (Hermes gateway, Claude
Code, Codex, anything that speaks MCP).

## Connecting

```jsonc
// Hermes gateway config
mcp_servers:
  task:
    url: https://task.example.com/org/<slug>/mcp
    headers: { Authorization: "Bearer <token>" }
```

- **Auth**: `Authorization: Bearer <token>` — either the static
  `TASK_MCP_TOKEN` the server was started with, or a real
  architect-auth session token (same rule as the watch bridge).
  `initialize` is deliberately unauthenticated so clients can discover
  the server before being told their token is wrong; everything else
  requires the bearer.
- **Orientation**: `initialize` returns `instructions` (org slug,
  current date/time, vault conventions, house rules). Clients fold it
  into the agent's system prompt — that text is how the agent knows it
  is inside Task.

## Plugin gating

Every tool belongs to a plugin (`core`, `scheduling`, `contacts`,
`email`, …) — the same `task_plugin::CATALOG` ids the vox router
mounts services by. For an org whose `org.toml` disables a plugin:

- `tools/list` **omits** that plugin's tools entirely;
- `tools/call` on one of them returns a tool-level error naming the
  plugin ("… the Scheduling plugin (`scheduling`) is disabled for this
  org"), not a protocol error — the model reads it and adapts;
- unknown tool names remain JSON-RPC `-32601` method-not-found.

This mirrors the wire exactly: a disabled plugin's vox services are
never mounted on the org router, so neither surface can reach them.

## The catalog

Descriptions in `tools/list` are written for the model (when to reach
for the tool, not just what it does) — this table is the human index.
Each write tool calls the same backend the equivalent vox method does,
so its effective permit posture is the wire method's permit
(`apps/task/server/src/permits.rs`); the right-hand column names it.

### Orientation & discovery (core)

| tool | does | wire permit |
|---|---|---|
| `task_context` | date/time, org, open-work counts | read `tasks/**` + `inbox/**` (+ `scheduling/**` when enabled) |
| `api_reference` | the org's ENTIRE vox surface: every service + method, permits, mounted flags; `service` arg expands one | none (build-static metadata, same body as `GET /org/{slug}/api`) |

### Tasks (core)

| tool | does | wire permit |
|---|---|---|
| `list_tasks` | filtered list (status / due window / project / title query) | read `tasks/**` (`task/query`) |
| `create_task` | quick-add parse of natural language | write `tasks/**` (`task/create`) |
| `update_task` | patch status / title / priority / due / scheduled / assignees | write `tasks/**` (`task/update`) |
| `claim_task` | atomic claim — one winner under parallel agents | write `tasks/**` (`task/try_claim`) |

### Projects / goals / milestones (core)

`list_projects`, `create_project`, `update_project` — read/write
`projects/**`; `list_goals`, `create_goal`, `update_goal` — read/write
`goals/**`; `list_milestones`, `create_milestone`, `update_milestone`
— read/write `milestones/**`. No delete tools by design (v1 exposes no
deletion anywhere).

### Inbox (core)

| tool | does | wire permit |
|---|---|---|
| `capture_inbox` | capture; lands as `suggested` unless `accepted: true` | write `inbox/**` (`inbox/upsert_inbox_item`) |
| `list_inbox` | today's review queue, or all items by status | read `inbox/**` |
| `process_inbox_item` | move an item to `open` / `processed` / `archived` | write `inbox/**` |

### Vault (core)

| tool | does | wire permit |
|---|---|---|
| `search_vault` | path search over the manifest | read `vault/**` (`vault-sync/manifest`) |
| `read_note` | one note's markdown | read `vault/{path}` |
| `append_note` | append-only add to an existing note | write `vault/{path}` |
| `write_note` | create **or replace** a note (flagged destructive in its description) | write `vault/{path}` |

### Calendar & scheduling (`scheduling` plugin)

| tool | does | wire permit |
|---|---|---|
| `list_events` / `create_event` / `reschedule_event` / `cancel_event` | calendar CRUD (cancel = audited delete) | `scheduling/events/**` |
| `get_day_plan` / `upsert_day_plan` | the time-blocked day (upsert replaces the whole day) | `scheduling/day-plans/**` |
| `list_open_slots` | free bookable slots for an event type | read `scheduling/slots/**` |
| `list_bookings` / `book_slot` / `cancel_booking` | bookings against the user's bookable event types | `scheduling/bookings/**` |

### Contacts (`contacts` plugin)

`list_contacts` (name/email/org search) — read `contacts/**`;
`upsert_contact` (create, or patch by id) — write `contacts/**`.

### Email (`email` plugin) — read + draft, **no send**

| tool | does | wire permit |
|---|---|---|
| `list_email_accounts` | accounts + their folders | read `email/**` |
| `list_envelopes` | newest-first summaries for one folder | read `email/**` |
| `read_email` | full message: headers, text body, attachment names | read `email/**` |
| `draft_email` | compose a new message **into Drafts** | write `email/**` (`email/append_draft`) |
| `draft_reply` | reply with correct To / `Re:` / threading headers, into Drafts | write `email/**` (`email/append_draft`) |

There is deliberately no send tool: `email/send` is an audited
outbound effect and stays human-initiated. Drafts wait in the user's
mail client. (The org's current maildir backend reports drafts as
unsupported until its phase-3 write path lands; the tools surface that
error verbatim.)

## Why there is no generic `invoke_service`

`api_reference` shows ~80 services; a natural follow-on is
`invoke_service(service, method, args_json)`. It was evaluated and
deliberately **not** built, because vox's dispatch machinery is typed
end-to-end:

- every args/response decode goes through a **phon compatibility
  decode program** built per (method, direction, reader type) from the
  *peer's* schema closure, exchanged per connection
  (`vox::schema_deser`) — there is no static wire encoding to target
  from JSON;
- the only server dispatch entry, `Handler::handle`, consumes a
  `SelfRef<RequestCall>` and a `DriverReplySink` that only a live
  connection driver constructs — there is no
  `call(MethodId, bytes)`/`call_json` seam;
- the typed client side (`establish::<Client>()`) is generated per
  service; vox 0.10 ships no dynamic/untyped client.

So a "generic" tool would in practice be a hand-written closure per
method — the curated catalog again, minus the good descriptions. If a
future vox grows a dynamic client (JSON → facet `Partial` via
`args_shape`, plus schema-exchange participation), wire it in
`mcp.rs` behind the same plugin gate and permit posture; the
descriptor registry (`permits::mounts()`) already carries everything
needed to describe it.

## Adding a tool

1. Add a `ToolDef` to `tool_catalog()` in `apps/task/server/src/mcp.rs`
   — name, **model-facing** description, JSON schema, owning `plugin`
   id (must be a `task_plugin::CATALOG` id; unit tests enforce it).
2. Add its match arm to `call_tool` — call the org backend directly
   (the same trait the vox service dispatches to), map errors through
   `backend_err`/`ToolFailure::Message` so the model gets actionable
   text.
3. Match the wire method's permit posture: no delete verbs, audited
   operations (sends, money) stay excluded or draft-first.
4. Tests: the catalog unit tests cover shape automatically; extend
   `tests/mcp_e2e.rs` when the tool warrants a round-trip.
