# workflows-orchestrator — shared agent-workflow primitives

**Status:** partially shipped — needs triage (2026-07-27). `features/task/workflows/{workflows-proto,workflows-orchestrator}` exist; scope completion was not verified.

**Target:** define the cross-domain primitives every concrete workflow uses (coding, writing, music, research, …). Provides the state machine, the agent attribution model, and the handoff/resume primitive that lets one agent pick up another's incomplete work without losing context.

Depends on `plans/issue-tracker-integration.md` for the `AgentRef` type — defined on `TaskInfo`'s `WorkflowAttrs` to avoid a circular dep (re-exported here).

**Subject type for the coding workflow is `task_proto::TaskId`** (TaskInfo with `WorkflowAttrs`); other workflows use their domain's primary entity (wiki: `PathBuf`, music: `KeyflowChart`, …).

## What we have today

- Per-domain ad-hoc workflows: `task wiki ingest`, `task wiki research-plans`, `task wiki review` already implement a workflow-shaped loop but not as a reusable primitive.
- `agent-queue` for scheduled / queued work (sqlite under each org).
- No `Activity` audit log entity. No `WorkSession` concept that survives across agent boundaries.
- No `Handoff` primitive — agent A blocking on a question + agent B resuming requires re-reading the entire context.

## Design

### Layout

```
features/workflows/
  workflows-proto/         # entities + traits
  workflows-orchestrator/  # state-machine driver + handoff/resume runtime
```

### Entities (`workflows-proto/src/lib.rs`)

```rust
#[derive(architect::Entity)]
#[architect(table = "work_sessions", vault = "workflows/sessions/", crdt = "loro")]
pub struct WorkSession {
    #[architect(id)] pub id: SessionId,                  // ULID, ordered
    #[architect(property)] pub workflow: WorkflowKind,   // coding | writing | music | …
    #[architect(property)] pub subject: SubjectRef,      // what's being worked on
    #[architect(property)] pub agent: AgentRef,
    #[architect(property)] pub status: SessionStatus,    // Active | Parked | Blocked | Finished | Cancelled
    #[architect(property)] pub started_at: DateTime<Utc>,
    #[architect(property, optional)] pub ended_at: Option<DateTime<Utc>>,
    #[architect(text, crdt = "loro::Text")] pub scratchpad: String,
}

pub enum WorkflowKind {
    Coding, Writing, Music, Research, Plugin, Custom(String),
}

pub enum SubjectRef {
    Issue(IssueId),
    WikiPage(PathBuf),
    KeyflowChart(PathBuf),
    Custom { kind: String, id: String },
}

#[derive(architect::Entity)]
#[architect(table = "transitions")]
pub struct Transition {
    #[architect(id)] pub id: TransitionId,
    #[architect(property)] pub session: SessionId,
    #[architect(property)] pub from: TransitionState,     // domain-specific enum-as-string
    #[architect(property)] pub to: TransitionState,
    #[architect(property)] pub actor: AgentRef,
    #[architect(property)] pub at: DateTime<Utc>,
    #[architect(property)] pub note: Option<String>,
}

#[derive(architect::Entity)]
#[architect(table = "activities")]
pub struct Activity {
    #[architect(id)] pub id: ActivityId,
    #[architect(property)] pub session: SessionId,
    #[architect(property)] pub kind: ActivityKind,        // Commit | Comment | StatusChange | ToolCall | Handoff
    #[architect(property)] pub actor: AgentRef,
    #[architect(property)] pub at: DateTime<Utc>,
    #[architect(property)] pub payload: serde_json::Value,
}

#[derive(architect::Entity)]
#[architect(table = "handoffs", vault = "workflows/handoffs/")]
pub struct Handoff {
    #[architect(id)] pub id: HandoffId,
    #[architect(property)] pub session: SessionId,
    #[architect(property)] pub from: AgentRef,
    #[architect(property, optional)] pub to: Option<AgentRef>,  // None = anyone
    #[architect(property)] pub reason: HandoffReason,            // BlockedExternal | NeedsHumanInput | OutOfScope | ContextLimit | …
    #[architect(text)] pub summary: String,                       // "where I left off" — markdown
    #[architect(text)] pub open_questions: String,                // bullet list
    #[architect(text)] pub recommended_next: String,              // bullet list
    #[architect(property)] pub status: HandoffStatus,             // Open | Claimed | Resolved | Cancelled
    #[architect(property)] pub created_at: DateTime<Utc>,
    #[architect(property, optional)] pub resolved_at: Option<DateTime<Utc>>,
}
```

### The `Workflow` trait (`workflows-proto/src/workflow.rs`)

```rust
#[async_trait]
pub trait Workflow: Send + Sync {
    type Subject: SubjectMarker;
    type State: WorkflowState;

    fn kind(&self) -> WorkflowKind;

    async fn ready(&self, agent: &AgentRef) -> Vec<Self::Subject>;

    async fn start(&mut self, subject: Self::Subject, agent: AgentRef)
        -> Result<WorkSession, WorkflowError>;

    async fn transition(&mut self, session: SessionId, to: Self::State, actor: AgentRef)
        -> Result<Transition, WorkflowError>;

    async fn record(&mut self, session: SessionId, activity: ActivityKind, payload: serde_json::Value)
        -> Result<Activity, WorkflowError>;

    async fn park(&mut self, session: SessionId, reason: HandoffReason, summary: String)
        -> Result<Handoff, WorkflowError>;

    async fn resume(&mut self, session: SessionId, agent: AgentRef)
        -> Result<ResumeContext, WorkflowError>;

    async fn finish(&mut self, session: SessionId, actor: AgentRef)
        -> Result<(), WorkflowError>;
}

pub struct ResumeContext {
    pub session: WorkSession,
    pub last_state: TransitionState,
    pub recent_activity: Vec<Activity>,        // last N events
    pub open_handoff: Option<Handoff>,
    pub related: Vec<RelatedRef>,              // wiki pages, issues, code symbols
    pub scratchpad: String,
}
```

### The orchestrator runtime (`workflows-orchestrator/`)

Drives the state machine + records every transition + enforces capability boundaries:

```rust
pub struct Orchestrator<W: Workflow> {
    workflow: W,
    capabilities: CapabilityRegistry,         // agent → allowed transitions
    activity_sink: ActivitySink,              // writes to architect-rpc
    notifications: NotificationBus,           // vox-streamed change events
}

impl<W: Workflow> Orchestrator<W> {
    pub async fn execute(&mut self, intent: Intent, actor: AgentRef) -> Result<Outcome, OrchestratorError> {
        self.capabilities.check(&actor, &intent)?;
        let outcome = self.workflow.handle(intent.clone(), actor.clone()).await?;
        self.activity_sink.record(&outcome).await?;
        self.notifications.publish(&outcome).await?;
        Ok(outcome)
    }
}
```

### Capability boundaries

```rust
pub enum Capability {
    Read,
    Comment,
    Transition(TransitionKind),  // allowed transitions, e.g. Triage→Backlog
    Create,
    Close,
    Merge,
    Custom(String),
}

pub struct AgentCapabilities {
    pub agent: AgentRef,
    pub workflow: WorkflowKind,
    pub allow: Vec<Capability>,
}
```

Examples:

```toml
# org.toml — capability config
[[agents]]
name = "triage-bot"
workflows = ["coding"]
allow = ["Read", "Transition(Triage→Backlog)", "Comment"]

[[agents]]
name = "deepen-bot"
workflows = ["writing"]
allow = ["Read", "Edit(body)", "Comment"]

[[agents]]
name = "claude-opus-4-7"
workflows = ["*"]
allow = ["*"]
```

### Notifications

Workflow events publish on the vox bus:

```rust
pub enum WorkflowEvent {
    Started   { session: SessionId, subject: SubjectRef, agent: AgentRef },
    Transitioned { session: SessionId, from: String, to: String, actor: AgentRef },
    Activity  { session: SessionId, kind: ActivityKind, actor: AgentRef },
    Parked    { session: SessionId, handoff: HandoffId },
    Resumed   { session: SessionId, agent: AgentRef },
    Finished  { session: SessionId, actor: AgentRef },
}
```

Agents subscribe via `task workflows watch`:

```bash
task workflows watch --filter "workflow=coding,subject=PROJ-142"
```

### CLI surface (`task workflows *`)

Mostly used by tooling; humans rarely call these directly. The per-domain workflows (`task code *`, `task wiki *`) wrap them.

```
task workflows list      [--agent A] [--status S]
task workflows show      <session-id>
task workflows resume    <session-id> --as-agent <name>
task workflows handoffs  [--workflow K] [--unclaimed]
task workflows claim     <handoff-id> --as-agent <name>
task workflows audit     <subject-ref>          # full activity history for a subject
```

### Phases

| Phase | Scope | Effort |
|---|---|---|
| **0** | Entities (`WorkSession`, `Transition`, `Activity`, `Handoff`) + DB migrations. | 2 days |
| **1** | `Workflow` trait + Orchestrator runtime. Capability registry. Activity sink. | 3 days |
| **2** | Handoff / resume primitive. `ResumeContext` assembly (last activity, related refs, scratchpad). | 3 days |
| **3** | Notifications via vox bus. `task workflows watch` stream. | 2 days |
| **4** | Capability config in `org.toml`. RBAC enforcement at the orchestrator boundary. | 2 days |

Total: ~12 days.

## Acceptance criteria

- [ ] `WorkSession`, `Transition`, `Activity`, `Handoff` entities compile and round-trip.
- [ ] `Workflow` trait is implementable: a stub workflow can `start → transition → record → park → resume → finish` and produces a coherent audit log.
- [ ] `Orchestrator` enforces capabilities: an agent without `Capability::Close` calling `finish` returns `OrchestratorError::Forbidden`.
- [ ] `ResumeContext` includes: last N=10 activities, the open Handoff, the scratchpad, related refs from wiki-graph.
- [ ] `task workflows watch` streams `WorkflowEvent` over vox; integration test confirms ordering + at-least-once delivery.
- [ ] Capability config in `org.toml` parses and is hot-reloadable.

## Out of scope

- Concrete workflows. This plan only ships the primitives; `plans/workflow-coding.md` builds the first concrete one.
- Web UI for workflow + handoff inspection. CLI + RPC only in this scope.
- Migration of existing ad-hoc workflows (wiki ingest / review queue) to the Workflow trait. Can come later; their current shape stays working.
- Multi-agent simultaneous sessions on the same subject (lock contention). Phase-1 model is one active session per subject; concurrent edits are sequenced.

## File-level breakdown

| File | Change |
|---|---|
| `features/workflows/workflows-proto/Cargo.toml` | New crate |
| `features/workflows/workflows-proto/src/lib.rs` | `WorkSession`, `Transition`, `Activity`, `Handoff` entities + `Workflow` trait |
| `features/workflows/workflows-proto/src/agent_ref.rs` | `AgentRef` enum (re-exported; defined in issue-proto) |
| `features/workflows/workflows-proto/src/capability.rs` | `Capability`, `AgentCapabilities` types |
| `features/workflows/workflows-orchestrator/Cargo.toml` | New crate |
| `features/workflows/workflows-orchestrator/src/lib.rs` | `Orchestrator` runtime |
| `features/workflows/workflows-orchestrator/src/sink.rs` | `ActivitySink` writing to architect-rpc |
| `features/workflows/workflows-orchestrator/src/notifications.rs` | vox event publishing |
| `features/workflows/workflows-orchestrator/src/resume.rs` | `ResumeContext` assembly (queries activity + wiki-graph) |
| `apps/cli/src/main.rs` | `task workflows *` subcommands |
| `org.toml.example` | `[[agents]]` capability section example |

## Related

- `plans/issue-tracker-integration.md` — defines `AgentRef`; the `Issue` entity references `WorkSession` for "currently being worked on by"
- `plans/workflow-coding.md` — the first concrete `Workflow` impl
- `Knowledge/wiki/concepts/wiki-graph.md` — `ResumeContext.related` queries this for context surfacing
- Memory: `architect-entity-default` — all entities above follow this rule
