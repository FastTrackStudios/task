# Agent Dispatch

**Status:** partially shipped — needs triage (2026-07-27). `features/task/agent/agent-dispatch` and `agent-tasks` exist; per-phase completion against this doc was not verified.

Dispatch any task note as an *agent task* — a unit of work an AI agent
picks up, runs, and reports back on. Modeled on
[`nousresearch/hermes-webui`](https://github.com/nousresearch/hermes-webui)'s
queue (`api/kanban_bridge.py`) — atomic claim, dispatcher-owned
`running` state, live event tail — but the domain model is named after
*what it is* (agent tasks in a queue), not after the kanban
visualization that's one possible rendering of it.

## Naming

Naming is deliberately visualization-neutral:

| Domain term | Not this |
|---|---|
| **Agent task** — one dispatchable unit of agent work | "Card" (kanban) |
| **Agent queue** — a grouping of agent tasks (per project, per agent profile, etc.) | "Board" (kanban) |
| **Status** — `triage / ready / running / blocked / done / archived` | "Column" (kanban) |
| **Dispatch** — the verb for handing a task off to an agent | (no kanban analog) |

A queue might be rendered as a kanban with status-columns, a flat
status-sorted list, a timeline by `claimed_at`, a per-agent swimlane,
or a graph by dependency edges. The model doesn't pick one. The
existing `view-kanban` crate is just one of the renderings.

The current `features/agent/agent-proto/src/kanban.rs` uses
kanban-coded names (`Board`, `Card`, `Column`). Renaming those is
part of slice 1.

## Why this is its own feature

Task notes (`features/task/task`) and agent tasks look superficially
similar — both have a title, status, dependencies — but they answer
different questions and need different stores.

| | Task note | Agent task |
|---|---|---|
| **What it is** | A thing *you* care about over time | A unit of work *an agent* will execute |
| **Storage** | `tasks/<slug>.md` in the vault | row in `agent-queues/<queue>.db` (SQLite) |
| **Lifecycle** | Years (recurring habits) or weeks (project work) | Hours to days |
| **Status owner** | The user | The dispatcher (`running` cannot be set manually) |
| **Concurrency** | Single-writer on a markdown file | Atomic `UPDATE WHERE claim_lock IS NULL` |
| **Persistence model** | Markdown is the source of truth | DB row is the source of truth; archive sweeps `done` |
| **Volume** | Bounded by what you promote | Could be thousands per project |
| **Example** | `brush-teeth-morning.md` with `complete_instances` going back years | "Port the view to Dioxus 0.7" — opened today, done tomorrow, archived next week |

Task notes already track high-frequency work via [TaskNotes-shape
recurrence](https://github.com/callumalpass/tasknotes): one file per
*identity* (e.g. brushing teeth), `recurrence:` RRule, and
`complete_instances: ["2026-05-21", "2026-05-22", ...]` accumulating
date strings forever. Decades of completions live in one file's flat
array — no file count explosion. So we do NOT need agent-tasks-as-files
for habit tracking; that's already solved.

What's not solved is **dispatchable execution units owned by an
agent**, with claim semantics, transcripts, and live UI. That's what
this feature adds, and it doesn't belong in markdown.

## Architecture

### Storage

- **Task notes**: unchanged. `tasks/*.md` with TaskNotes-shape
  frontmatter. Includes recurring habits, project work, capture-style
  inbox items.
- **Agent queues**: one SQLite file per queue, under
  `<vault_parent>/agent-queues/<slug>.db`. NOT in the vault — the
  vault watcher would trip on every status flip, and the DB isn't
  human-readable so it doesn't belong in the markdown world.
- The bridge: each agent task carries an optional `source_task` field
  (relative path to the task note). Each task note can carry a
  `dispatched_agent_tasks:` list pointing back. Either side answers
  "what's the link?" without a join table.

### Crates

```
features/agent/
├── agent-proto/      (exists) — types + traits; rename Board→Queue,
│                                Card→AgentTask, Column→Status
├── agent-codex/      (exists)
├── agent-wiki/       (exists)
├── agent-tasks/      NEW — SQLite impl of agent-proto::tasks traits
├── agent-dispatch/   NEW — task-note ↔ agent-task bridge, dispatch
│                          policy, completion writeback, archive sweep
```

`view-kanban` (existing crate) consumes the new model as one
rendering. Other views (timeline, agenda, graph) are future
crates and not part of this plan.

### `agent-tasks` (the data layer)

Implements the renamed traits in `agent-proto/src/service/tasks.rs`
(was `kanban.rs`). Owns its own SQLite via `rusqlite` (consistent
with `persistence-sqlite`).

Schema is essentially Hermes':

```sql
CREATE TABLE agent_tasks (
  id TEXT PRIMARY KEY,                -- ULID
  queue_id TEXT NOT NULL,
  status TEXT NOT NULL,               -- triage|ready|running|blocked|done|archived
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  priority INTEGER NOT NULL DEFAULT 2,
  labels TEXT NOT NULL DEFAULT '[]',  -- JSON array
  source_task TEXT,                   -- vault-relative path to .md, NULL if standalone
  project TEXT,                       -- vault-relative path to project note
  agent_profile TEXT,                 -- which agent backend should claim (codex|hermes|...)
  claim_lock TEXT,                    -- worker id holding the claim, NULL if free
  claim_expires INTEGER,              -- unix-ms; dispatcher reclaims if elapsed
  worker_pid INTEGER,
  result_blob TEXT,                   -- JSON transcript / output on done
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived_at INTEGER
);

CREATE TABLE agent_task_comments (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_task_id TEXT NOT NULL,
  author TEXT NOT NULL,               -- "user" or agent profile name
  body TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE agent_task_links (
  from_task TEXT NOT NULL,
  to_task TEXT NOT NULL,
  kind TEXT NOT NULL,                 -- blocks|related|spawned_from
  PRIMARY KEY (from_task, to_task, kind)
);

CREATE TABLE queue_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_task_id TEXT,
  kind TEXT NOT NULL,                 -- created|status_changed|claimed|comment|done|archived
  payload TEXT NOT NULL,              -- JSON
  ts INTEGER NOT NULL
);
```

Status set follows Hermes verbatim:
`triage → ready → running → blocked → done → archived`.
The `running` status is dispatcher-owned: setting it via the normal
API returns an error (must go through `claim`). This matches Hermes'
contract and means any UI — kanban or otherwise — can't accidentally
clobber an in-flight run.

`queue_events` is the source for the live update stream — the vox
subscription tails it. Notice this is named after the *queue*, not
after any particular view of it. Whether the UI is a kanban board, a
flat list, or a graph, it consumes the same event stream.

### `agent-dispatch` (the bridge)

No new wire types beyond `agent-proto`; pure orchestration.

Public surface:

```rust
pub fn dispatch(
    task: &task::Task,
    queue: QueueId,
    options: DispatchOptions,    // agent_profile, priority, initial status
) -> Result<AgentTask, DispatchError>;

pub fn complete_agent_task(
    id: &AgentTaskId,
    result: AgentTaskResult,     // transcript + exit status + artifacts
) -> Result<(), DispatchError>;

pub fn archive_sweep(
    queue: QueueId,
    older_than: Duration,
) -> Result<usize, DispatchError>;

pub fn weekly_digest(
    queue: QueueId,
    week_of: NaiveDate,
) -> Result<String, DispatchError>;  // returns markdown blob
```

**`dispatch` writes both sides**:

1. INSERT into `agent_tasks` with `source_task` = `task.path`.
2. Patch the task note's frontmatter: append the agent task id to
   `dispatched_agent_tasks: [...]`.
3. Both writes happen under `agent-dispatch`'s write lock so a
   crash mid-way doesn't leave dangling references. On replay,
   `agent-dispatch::reconcile()` walks task notes + agent tasks,
   flags orphans, and the user picks (most will be "delete the
   stale agent task").

**`complete_agent_task`**:

1. Agent task moves to `done`, `result_blob` populated.
2. If `source_task` is set:
   - For **one-shot tasks**: source task status → `done`,
     `completed_date` stamped, append a single bullet to the body:
     `- 2026-05-22: shipped by agent · [task-abc123](agent-queue://...)`.
   - For **recurring tasks**: today's date appended to
     `complete_instances`. The agent task is just one occurrence of
     an ongoing habit (e.g. agent ran "daily-news-digest" today).

**`archive_sweep`** runs nightly (or on demand): agent tasks in `done`
for more than N days move to `archived` and stop showing in default
queue views. Audit history stays in `queue_events` indefinitely.

**`weekly_digest`** is what turns ephemeral agent tasks into a durable
record. Reads archived agent tasks from the last week, groups by
project, emits a markdown blob the user can paste into a project note
(or that a Daily note template can include via a query).

### Visualization — first impl: `view-kanban` (existing crate)

`view-kanban` currently kanbans task notes. Add a data-source enum:

```rust
pub enum KanbanSource {
    Tasks { vault_root: PathBuf, filter: TaskFilter },
    AgentQueue { db_path: PathBuf, queue_id: String },
}
```

Both render through the same DOM/component tree — status-grouped
columns, draggable cards, status badges. What differs:

- **Tasks source**: drag = rewrite frontmatter. Idempotent. No claim.
- **AgentQueue source**: drag respects status transitions (can't
  drag *into* `running` from the UI). Agent tasks show a worker
  badge + a transcript drawer that streams from `result_blob`. Live
  updates via vox subscription tailing `queue_events`.

The "Open agent queue" button on a project page
(`Projects/Architect.md`) opens this view with
`AgentQueue { db_path: ".../architect.db", … }`. A "Dispatch to
agents" command on a task note opens a queue picker, then calls
`agent_dispatch::dispatch(...)`.

**Other visualizations later** — none of these are blocked by this
plan, but worth naming so the model stays neutral enough to support
them:

- A *timeline view* showing `claimed_at → done_at` bars per agent task.
- An *agenda view* listing all agent tasks scheduled for today, across
  every queue.
- A *graph view* showing dependency edges between agent tasks.

### Live updates

vox subscription on `agent-queue://<slug>/events` returns
`QueueEvent { agent_task_id, kind, payload, ts }` records, streamed
from `queue_events` (long-polling via the `id` watermark — same
pattern Hermes' SSE uses but over our existing vox transport).

Any open view, on any client, observing a queue re-renders on each
event. This is the visible difference from the file-watcher path on
the tasks side: agent queues update in real time even when the
underlying state changes from a worker on another machine.

## Walkthroughs

### Dispatching a project task

1. Task note open: `tasks/port-view-to-dx07.md`. Status `open`,
   projects `["[[Architect]]"]`.
2. Cmd palette → "Dispatch to agents". Picker pre-fills with the
   `architect` queue (from the project link). Choose agent profile
   (codex, hermes, etc.) and priority.
3. Agent task created in `architect.db`, status `triage`. Task note
   gets `dispatched_agent_tasks: ["task-abc123"]` appended.
4. User reviews + advances to `ready`. Dispatcher claims, moves to
   `running`. Agent runs.
5. Agent calls `complete_agent_task(task-abc123, result)`. Status
   moves to `done`. Source task note status → `done`, body appended
   with a completion bullet.
6. Agent task sits in `done` for 7 days, then archives. Project's
   weekly digest catches it.

### Recurring task as agent work

Task `tasks/daily-news-digest.md`:

```yaml
title: Daily news digest
type: task
status: open
recurrence: "DTSTART:20260501T070000Z;FREQ=DAILY"
scheduled: 2026-05-22T07:00
complete_instances: ["2026-05-20", "2026-05-21"]
agent_profile: hermes-research
```

The `agent_profile` field marks it as agent-runnable. A daily cron
in `agent-dispatch::schedule_recurring()` walks task notes with
`agent_profile` set whose `scheduled` is due, and dispatches each as
an agent task. On completion, `complete_agent_task` adds today's
date to the task's `complete_instances`, advances `scheduled`, and
archives the agent task same-day (no point keeping a `done` digest
agent task around for a week — the artifact is the digest itself,
stored wherever the agent wrote it, and the dated completion in the
task note's array).

This gives precise tracking of high-frequency agent work over time:
the task note answers "how many times has the daily-news-digest
agent run successfully this year?" with
`complete_instances.len()` in the relevant year.

### Human recurring task — no agent involvement

`tasks/brush-teeth-morning.md`: same recurrence machinery, no
`agent_profile`, never dispatched. The task crate handles
completion in-place — Agent Dispatch is irrelevant here.
**Important point: recurring habits don't need to be agent-dispatched
to be first-class.** Most won't be.

## Open questions

1. **Per-queue agent profile vs. per-agent-task?** A queue could
   declare a default profile (this queue runs Codex); agent tasks
   inherit unless overridden. Hermes' kanban is profile-agnostic —
   workers self-select.
2. **Where do queue DBs actually live?** Sibling to vault, or
   nested under `.task/` inside the vault and gitignored? Argument
   for sibling: the vault should stay portable across machines that
   may not even have agents configured. Argument for inside: makes
   queue state travel with the vault for backup.
   Probably: **sibling, with a settable path**, defaulting to
   `<vault>/../agent-queues/`.
3. **Schema migrations.** Hermes does ad-hoc `ALTER TABLE`. We can
   do the same since DBs are small (single user, single queue) and
   migrations cheap.
4. **Multi-machine ownership.** If two machines have a worker
   pointed at the same queue file, claim contention is naturally
   resolved by SQLite (the `UPDATE … WHERE claim_lock IS NULL` is
   atomic). But the queue DB itself isn't replicated — it'd need
   to go through vault-sync's file-replication path or be hosted
   on the server with the dispatcher. **Defer to slice 5.**

## Slice plan

**Slice 1 — agent-tasks + proto rename** (~3 days)
- Rename `agent-proto/src/kanban.rs` → `tasks.rs`. `Board` →
  `Queue`, `Card` → `AgentTask`, `Column` → `Status`,
  `BoardView` → gone (was UI-shaped — the view layer projects
  its own shape), `BoardFilter` → `QueueFilter`, `CardLink` →
  `AgentTaskLink`, `CardComment` → `AgentTaskComment`.
- `features/agent/agent-tasks` crate.
- Schema + `rusqlite` impl of `Queues`, `AgentTasks`,
  `AgentTaskComments`, `AgentTaskLinks` traits.
- `queue_events` tailer with watermark cursor (no vox yet).
- Unit tests cover claim atomicity, archive sweep, dependency
  prevention of move-to-done while blocked.

**Slice 2 — dispatch bridge** (~2 days)
- `features/agent/agent-dispatch` crate.
- `dispatch()`, `complete_agent_task()`, `archive_sweep()`.
- Add `source_task: Option<String>` to `AgentTask` proto (additive).
- Add `dispatched_agent_tasks: Vec<String>` to `task::Task` proto
  (additive).
- Reconcile pass for crash recovery.

**Slice 3 — first view** (~3 days)
- `view-kanban`: add `KanbanSource` enum, agent-queue branch.
- Live update via vox tailer on `queue_events`.
- Worker badge, transcript drawer, dispatch button on task pages.

**Slice 4 — recurring agent runs + digest** (~2 days)
- `schedule_recurring()` daily cron on tasks with `agent_profile`.
- `weekly_digest()` synth blob.
- Project-page widget that renders the current week's digest
  inline (queries the queue, doesn't store the result — re-renders
  each open).

**Slice 5 — multi-machine** (deferred)
- Vault-sync replicates `agent-queues/*.db` OR move dispatcher to
  the server and clients talk to it over vox.

## Server-side dispatcher (decided)

**The dispatcher runs on the server, not the client.** Clients call
vox RPCs (`dispatch`, `list_queue`, `subscribe_events`) and the server
owns the SQLite DBs, the agent backends (codex, hermes, …), and the
claim/run loop. This:

- Makes multi-machine work out of the box — every client sees the
  same queue state because there's one source of truth.
- Puts the long-running agent processes on the box with the right
  credentials (API keys, codex binary, etc.) instead of every laptop.
- Keeps the SQLite file off client disks where it'd get out of sync.
- Matches how `architect-rpc` already routes everything else.

Net change to slices:

- `agent-tasks` exposes its `Queues` / `AgentTasks` / etc. impls
  behind an `#[architect::rpc]` trait in `agent-proto` so the same
  surface works locally (tests) and over vox (production).
- `apps/server` mounts the dispatch service alongside `VaultSync`.
- The client never opens a `.db` file. `KanbanSource::AgentQueue`
  carries a `QueueClient` (vox handle) instead of `db_path`.
- Slice 5 collapses: multi-machine is *the* deployment.
