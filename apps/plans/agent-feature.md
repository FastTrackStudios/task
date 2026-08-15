# agent feature

**Status:** partially shipped — needs triage (2026-07-27). `agent-{proto,wiki,tasks,hermes,codex,inbox,dispatch}` all exist under `features/task/agent/`; which of this doc's slices are actually closed was not verified.

LLM-agent integration. Models projects, sessions, threads,
messages, tools, approvals, kanban, and the streaming
`AgentEvent` union over an `#[architect::rpc] trait
Agents`. Backends slot in as siblings (Hermes
in-process, Codex CLI monitor, Claude CLI bridge, Pi,
custom). Binding crates layer per-feature prompts +
parsers on top (`agent-wiki` drives `wiki-proto`; future
`agent-task` drives the task feature).

Synthesized from three deep dives:
- [hermes-webui](https://github.com/nesquena/hermes-webui)
  — in-process Hermes UI (Python + SSE).
- [CodexMonitor](https://github.com/Dimillian/CodexMonitor)
  — Tauri app that monitors external Codex CLI logs.
- [llm_wiki](https://github.com/nashsu/llm_wiki) — for the
  prompt templates carried in `agent-wiki`.

## Two backend shapes

| Shape              | Example         | `dispatch_turn` semantics                  |
|--------------------|-----------------|--------------------------------------------|
| `InProcess`        | embedded Hermes | Async; backend owns the agent runtime      |
| `ExternalMonitor`  | Codex CLI       | Usually `Unsupported`; events come from logs |
| `CliBridge`        | claude CLI      | Spawns CLI per turn; parses stdout         |
| `Http`             | hosted Hermes / peer Task server | Standard SSE proxy        |

`Agents` is the same trait for all four; the
implementation varies. UIs and CLIs depend only on the trait.

## Crate plan

```text
features/agent/
├── agent-proto/   ✅ shipped — wire contract
├── agent-wiki/    ✅ shipped — wiki bindings (prompts + parsers)
├── agent-codex/   🚧 in flight — vendor + skeleton shipped; Agents impl queued
├── agent-hermes/  ⏳ future — in-process Hermes backend
├── agent-task/    ⏳ future — task/kanban bindings
├── agent-cli/     ⏳ future — `task agent ...` CLI subcommands
├── agent-ui/      ⏳ future — Dioxus chat + kanban + diff viewer
└── agent/         ⏳ future — facade re-export
```

> Dropped: `agent-claude-cli`. Claude usage on Task happens
> through the user's editor / CLI, not as an in-app agent
> backend. If that changes later, add it back.

## Shipped: `agent-proto`

Module map (`features/agent/agent-proto/src/`):

| File           | What                                                              |
|----------------|-------------------------------------------------------------------|
| `service.rs`   | `Agents` trait + architect-emitted client.                  |
| `backend.rs`   | `AgentBackend`, `BackendKind`, `BackendHealth`.                   |
| `profile.rs`   | `Profile`, `Personality`, `ModelConfig`, `ToolsetConfig`, `McpServerSpec`. |
| `project.rs`   | `Project`, `GitContext`, `ProjectSettings`.                       |
| `session.rs`   | `Session`, `SessionStatus`, `SourceTag`, `PendingTurn`, `UsageStats`, `CompressionState`, `WorktreeBacking`, `ComposerDraft`. |
| `message.rs`   | `Message`, `Role`, `ContentBlock` (multimodal: text/image/tool_use/tool_result/code). |
| `tool.rs`      | `ToolCall`, `ToolStatus`, `FileChange`, `CollabRouting`.          |
| `reasoning.rs` | `ReasoningBlock`.                                                 |
| `attachment.rs`| `AttachmentRef`, `Attachment`, `AttachmentKind`.                  |
| `approval.rs`  | `Approval`, `ApprovalKind`, `RiskLevel`, `ApprovalDecision`.      |
| `question.rs`  | `QuestionRequest`, `Question`, `QuestionOption`, `QuestionAnswer`.|
| `kanban.rs`    | `Board`, `BoardView`, `Card`, `CardLink`, `CardComment`, `BoardFilter`. |
| `event.rs`     | `AgentEvent` streaming union.                                     |
| `paths.rs`     | Disk layout for state-keeping backends.                           |
| `error.rs`     | `AgentError`.                                                     |

`Agents` exposes ~50 methods: backend / profile /
project CRUD, session lifecycle + import-from-external,
turn dispatch + cancel + resume, message + tool + reasoning
+ attachment read, approval + question resolution, full
kanban CRUD, three subscription channels
(`session` / `board` / `global`).

## Shipped: `agent-wiki`

Binding library. Carries the prompt templates ported from
llm_wiki + parser signatures.

| File              | What                                                             |
|-------------------|------------------------------------------------------------------|
| `prompts.rs`      | 10 prompt constants loaded from `templates/*.txt`. `render()` helper for `{key}` substitution. |
| `templates/*.txt` | Verbatim ports of llm_wiki's `src/lib/{ingest,deep-research,lint,sweep-reviews,optimize-research-topic,dedup,vision-caption,output-language}.ts` prompt strings. |
| `parsers.rs`      | Signatures + types for FILE/REVIEW/LINT/JSON parsers. Bodies are `todo!()` until first backend lands. |
| `bridge.rs`       | Orchestration helpers — `run_ingest`, `run_lint`, `run_propose_research`, `run_sweep_reviews`, `run_dedup_detect`, `run_dedup_merge`. Signatures shipped, bodies `todo!()`. |

## Slices

### 1. ✅ Proto + wiki bindings (this commit)

`agent-proto` + `agent-wiki` compile cleanly.

### 2. `agent-codex` — first concrete backend

CodexMonitor already ships **1388 lines of working Rust**
that drive `codex app-server` (the official Codex daemon)
over JSON-RPC stdio. See
`~/Development/research/CodexMonitor/src-tauri/src/backend/app_server.rs`.

#### 2a. ✅ Vendor + skeleton (shipped)

Vendored modules now live under
`features/agent/agent-codex/vendor/` (mounted via
`#[path = "../vendor/mod.rs"] mod vendor;` so the directory
stays at the crate root for visible attribution):

- `app_server.rs` (1388 LOC) — verbatim, only import paths
  rewritten (`crate::backend::*` → `super::*`).
- `events.rs` — trimmed to just `AppServerEvent` + the
  `emit_app_server_event` half of `EventSink` (terminal
  events stripped).
- `process_core.rs` — verbatim; provides `tokio_command`,
  `kill_child_process_tree`, Windows cmd-wrapper helpers.
- `args.rs` — verbatim, `parse_codex_args` +
  `resolve_workspace_codex_args`.
- `types.rs` — minimal shim. CodexMonitor's `types.rs` is
  1418 LOC of UI config; we only need `WorkspaceEntry`,
  `WorkspaceKind`, `WorkspaceSettings`, `AppSettings` (~30
  LOC). Everything else is intentionally absent.

Skeleton wrapper at `src/lib.rs`:

- `BroadcastSink` — implements `EventSink` over
  `tokio::sync::broadcast` so multiple subscribers can tap
  the same Codex event stream.
- `CodexBackend` — top-level handle. Owns the broadcast
  sink + a registry of per-workspace `WorkspaceSession`
  handles.
- `subscribe_raw()` — for now, callers can subscribe to
  the unfiltered `AppServerEvent` firehose. The trait-shaped
  `subscribe_session` arrives in 2b.

External deps pinned: `tokio` (full), `serde`, `serde_json`,
`shell-words`, `tracing`, `chrono`, `thiserror`, plus
`agent-proto` as the trait surface.

#### 2b. ✅ Chat demo + CLI wiring (shipped)

- `agent_codex::chat()` spawns `codex app-server`, sends
  `thread/start` + `turn/start`, returns a
  `Stream<AgentEvent>` filtered to that workspace's events.
- `agent_codex::translate` maps the daemon's
  `item/agentMessage/delta` /`item/reasoning/delta` /
  `turn/completed` / `turn/error` notifications to typed
  events.
- `task-cli` exposes `task agent chat` with `--workspace`,
  `--model`, `--effort`, `--access-mode`, `--codex-bin`,
  `--codex-home`, `--timeout-secs`. Set
  `AGENT_CODEX_DEBUG=1` to dump the raw `AppServerEvent`
  firehose.
- Working command:
  ```bash
  task agent chat -w examples/vault \
    "Reply with exactly the word PONG."
  # → PONG
  ```

#### 2c. ⏳ Full Agents impl

Next commit. Translation layer that turns JSON-RPC messages
into `agent_proto` shapes:
   - mapping `AgentBackend::kind = ExternalMonitor` ↔
     `CliBridge` (Codex supports both modes — read-only logs
     + active subprocess dispatch).
   - translating CodexMonitor's `AppServerEvent` →
     `agent_proto::AgentEvent`.
   - translating CodexMonitor's `WorkspaceInfo` →
     `agent_proto::Project`, `ThreadSummary` → `Session`,
     `ConversationItem` (8-variant union) → `Message` +
     `ToolCall` + `ReasoningBlock` + `Approval` +
     `QuestionRequest`.
3. `dispatch_turn` works end-to-end (spawn or attach to
   `codex app-server`, send a `start_thread` RPC, stream the
   response events).
4. `import_external_session` reads a Codex log file and
   materializes a `Session` + messages without spawning the
   daemon — useful for archived sessions.

Once shipped, this is the first usable agent backend and
gives us the surface to validate `agent-wiki`'s ingest /
lint / dedup flows end-to-end. **The wiki feature reaches
full llm_wiki parity at the end of this slice** — agent
loops are real, prompts are real, output parsing is real.

### 3. `agent-wiki` parser bodies + bridge wiring

With Codex driving real agent loops, port the strict
parsers from llm_wiki's TypeScript:

- `parse_ingest_blocks` — `---FILE:` / `---REVIEW:` block
  parser; the response is dropped if the first character
  isn't `-`.
- `parse_lint_blocks` — `---LINT: type | severity | title---`
  blocks.
- `parse_dedup_groups`, `parse_sweep_resolved` — strict JSON
  (no markdown fences).
- `parse_research_plan` — exactly-4-line `TOPIC:` + `QUERY:`
  format.

Then wire `bridge::run_ingest` / `run_lint` /
`run_propose_research` / `run_sweep_reviews` /
`run_dedup_*` so a single function call drives one full
pipeline. **At the end of this slice, Task has llm_wiki
parity** — same prompts, same parsers, same flows, against
the same `Wiki/raw/sources/` shape.

### 4. `agent-hermes` (in-process)

The big one. Embeds Hermes runtime (Rust SDK when
available; shell out to the Python entrypoint as fallback).
Streams events via `subscribe_session`. Owns approvals +
questions + kanban end-to-end.

This is where personalities, MCP servers, multi-profile
support, composer drafts, and the run-journal SSE replay
all live. After this slice, Task can act as a full Hermes
WebUI replacement (single-user).

### 5. `agent-cli` (`task agent ...`)

CLI surface — `task agent session list`,
`task agent dispatch <msg>`, `task agent kanban list`,
`task agent ingest <source-path>` (delegates to
`agent_wiki::bridge::run_ingest`).

### 6. `agent-ui` (Dioxus)

Chat view, kanban board, diff viewer for tool changes,
approval dialog, session sidebar. Uses
`subscribe_session` for live updates.

### 7. `agent-task` (binding)

Sister to `agent-wiki` — drives the future task feature's
proto from agent loops. Same shape: prompt templates +
parsers + bridge.

## On-disk layout

Mirrors Hermes:

```text
<state>/agent/
├── backends.json
├── profiles/<id>/
│   ├── config.json
│   ├── personalities/
│   └── secrets.enc
├── projects.json
├── sessions/<session-id>.json
├── messages/<session-id>/<message-id>.json
├── attachments/<sha256>
├── tools/<session-id>/<tool-call-id>.json
├── approvals/<session-id>/<approval-id>.json
├── questions/<session-id>/<request-id>.json
├── boards/<board-id>.json
├── boards/cards/<card-id>.json
├── boards/links.json
├── boards/comments/<card-id>.json
└── run_journal.sqlite   ← SSE replay for crash recovery
```

Path constants live in
`agent_proto::paths`. Backends are free to swap storage
(SQLite, Sled, remote object store) — paths are just
defaults.

## Open questions

- **Hermes embedding** — does Hermes ship a Rust SDK, or
  do we spawn its Python entrypoint? Affects `agent-hermes`
  shape considerably.
- **Multi-tenancy** — Hermes is single-user. Task may
  eventually want per-user agent state; defer until needed.
- **Tool-use schema** — Anthropic + OpenAI use different
  shapes. We've abstracted to `ContentBlock::ToolUse {
  input_json }`, but backends will need translation layers.
- **Subagent delegation** — `CollabRouting` is in the
  trait but the loop semantics aren't pinned down. First
  Hermes integration will likely shape this.
- **Federation** — peer Task servers exposing
  `Agents` over HTTP/WS would let agents collaborate
  across vaults. Out of scope until the wiki feature lands
  federation first; same trait can be reused.

## Why this and not a thinner abstraction

A simple `chat(msg) -> stream<event>` would work for one
backend. The full surface buys:

- **Backend pluggability** — Hermes, Codex, Claude CLI all
  fit behind the same trait.
- **External-monitor parity** — read-only backends look
  the same to UIs.
- **Per-feature bindings** — `agent-wiki` (and future
  `agent-task`, etc.) layer cleanly without coupling
  agent-proto to any specific app domain.
- **Reuse with `architect::rpc`** — once we want remote
  agents, the same trait moves over vox without code
  duplication.
