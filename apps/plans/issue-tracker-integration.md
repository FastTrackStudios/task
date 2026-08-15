# issue-tracker-integration — Linear-style work model + GitHub/Forgejo sync

**Status:** partially shipped — needs triage (2026-07-27). `task issue` exists in the CLI and the workstream / cycle / milestone / label slices are built; bidirectional forge sync completeness was not verified.

**Target:** extend `TaskInfo` with Linear-style work attributes (workspace / cycle / project / estimate / blockers / agent-attribution), finish the partially-built `git-{proto,config,github}` stack, and add a `git-forgejo` backend. Underpins the `task code` (workflow-coding) work; ships as a usable bidirectional issue tracker first.

## What we have today

- `architect::Entity` schema-first persistence pattern, established across `features/{task,project,meal,pantry,...}`.
- Per-org isolation under `~/.task/orgs/<slug>/` with `org.toml`.
- `vox` transport for cross-device sync of architect entities.
- **`features/task/task/`** — `TaskInfo` architect Entity, markdown frontmatter at `tasks/<slug>.md` is the source of truth (TaskNotes-shape so Obsidian vaults round-trip). Has `Status` + `Priority` already; missing the rest of the Linear primitives.
- **`features/git/git-proto/`** — done. Three `#[architect::rpc]` traits: `RepoCatalog`, `IssueTracker`, `ReviewSurface`. DTOs for `Issue` / `Comment` / `PullRequest` / `Review`. `Forge` enum supports GitHub + Forgejo.
- **`features/git/git-config/`** — partial. `RepoBinding` (project → repo) + `IssueLink` (task → forge issue/PR), in-memory store. SQLite persistence TBD.
- **`features/git/git-github/`** — partial. `Backend` impls all three traits via `octocrab`. `list_issues` + `get_issue` wired; rest are `todo!()`.
- **No `git-forgejo` backend yet.** Authentication via SSH works; HTTPS+token Forgejo API not yet wrapped.
- Existing `Project` type at the org level (personal/work realm) — name collides with the Linear sense.

## Direction: extend TaskInfo, don't create a parallel Issue entity

`TaskInfo` is already the canonical unit of work in Task. Layering Linear-style fields onto it (in a nested `WorkflowAttrs` struct so the surface stays scoped) keeps **one issue/task type** with one source-of-truth. Forge issues link to TaskInfo via the existing `IssueLink`; TaskInfo wins on conflict for Task-only fields, forge wins for fields it owns.

## Design

### Extend `TaskInfo` with `WorkflowAttrs` (`features/task/task/src/model.rs`)

```rust
pub struct TaskInfo {
    // ... existing fields (id, title, status, priority, due, tags, etc.) ...
    pub workflow: Option<WorkflowAttrs>,
}

#[derive(Debug, Clone, Default, Facet, Serialize, Deserialize)]
pub struct WorkflowAttrs {
    pub workspace: Option<WorkspaceId>,
    pub cycle: Option<CycleId>,
    pub project: Option<ProjectId>,            // Linear-sense; old org Project → Domain
    pub estimate: Option<Estimate>,
    pub assignees: Vec<AgentRef>,              // who owns this; can be human or agent
    pub blockers: Vec<TaskId>,                 // hard deps; this task is blocked until each closes
    pub relates_to: Vec<TaskId>,               // soft links
    pub session: Option<SessionId>,            // currently-active WorkSession from workflows-proto
}

pub enum Estimate { XS, S, M, L, XL, Points(u8) }
pub enum AgentRef {
    Human { user_id: String },
    Agent { name: String, model_version: String },
}
```

`WorkflowAttrs` is serialized as nested YAML under a `workflow:` frontmatter key, so existing TaskNotes-shaped pages (without the key) keep round-tripping unchanged.

New sibling architect Entities:

| Entity | Crate | Purpose |
|---|---|---|
| `Workspace` | `features/workspace/workspace-proto/` (new) | Container for a set of synced repos + their tasks + cycles + labels |
| `Project` (new sense) | `features/project-roadmap/proto/` (new) | Multi-task initiative; old org-level `Project` renamed → `Domain` |
| `Cycle` | `features/cycle/` (already exists; verify shape matches Linear sense) | Time-boxed sprint with auto-rollover |
| `Label` | `features/label/label-proto/` (new) | Tag with name + color, scoped to a workspace |
| `View` | YAML under `vault/views/<name>.yml` | Saved filter; no architect entity needed (filesystem-native) |

`Comment` is *not* a new entity — comments on tasks live in `TaskInfo` already; comments on forge issues are accessed via `git-proto::IssueTracker::list_comments`. The split is intentional.

### Tracker trait — already exists as `git-proto::IssueTracker` + siblings

The trait surface my original plan called `Tracker` is already three `#[architect::rpc]` traits in `git-proto`:

- `RepoCatalog` — list/get repos
- `IssueTracker` — issue CRUD, comments, labels, assignees, milestones, state, **`subscribe`** for live updates
- `ReviewSurface` — PR CRUD, review threads, requested reviewers, merge

Done. The work that remains is **finishing the backends**:

- `git-github` — fill in the `todo!()` stubs (`create_issue`, `update_issue`, `list_comments`, `add_comment`, `subscribe`, all of `ReviewSurface`). Bridge to webhooks via Forgejo/GitHub event-delivery in Phase 3.
- `git-forgejo` — new crate. Mirror `git-github` shape over the Gitea-compatible REST API. Auth via PAT in keychain.

### Sync conflict model (operates over TaskInfo ↔ forge Issue)

Per-field source-of-truth split between TaskInfo and the forge issue it's linked to via `git_config::IssueLink`. Last-writer-wins with full audit log:

| Field | Wins on conflict | Why |
|---|---|---|
| `title`, `body`, `status (open/closed projection)` | Forge | The substrate dictates these |
| `assignees`, `labels-present-on-forge`, `milestone` | Forge | Forge models these natively |
| `priority`, `cycle`, `project (Linear-sense)`, `estimate`, `agent-attribution`, `task-only-labels` | Task | Forge doesn't model these |

Implementation: each `TaskInfo` field carries a `last_modified_by: AgentRef + Timestamp` micro-vector-clock. Sync resolves per-field; full audit log of every transition persists in the `Activity` table (defined in `plans/workflows-orchestrator.md`).

### Org config (`org.toml` extension)

```toml
[trackers.github]
type = "github"
default_branch_per_repo = "main"
keychain_key = "task.github.token.codywright"  # token retrieved via secret-service

[[trackers.github.repos]]
slug = "FastTrackStudios/task"
workspace = "task-core"

[[trackers.github.repos]]
slug = "FastTrackStudios/editor"
workspace = "task-core"

[trackers.forgejo]
type = "forgejo"
host = "git.starcommand.live"
keychain_key = "task.forgejo.token.codywright"

[[trackers.forgejo.repos]]
slug = "codywright/moire"
workspace = "music-tools"
```

### CLI surface (`task issue *` extends existing `task task *`)

Task already has `task task *` for TaskInfo CRUD. The new `task issue *` verbs are the **workflow-attribute-aware** subset:

```
task issue list      [--workspace W] [--status S] [--assignee A] [--cycle C]
task issue show      <id>                       # incl. linked forge issue summary
task issue create    --workspace W --title T [--body -|file] [--priority P]
task issue assign    <id> <agent-ref>
task issue link      <id> blocks|relates <other-id>
task issue link-forge <id> <repo> <number> [--kind issue|pr]  # bind to forge
task issue sync      [--workspace W]            # bidirectional sync
task issue watch     [--workspace W]            # vox stream over IssueTracker::subscribe
```

### Phases

| Phase | Scope | Effort |
|---|---|---|
| **0** | `WorkflowAttrs` on TaskInfo (frontmatter round-trip). New `Workspace` + `Project (new sense)` + `Label` entities. DB migrations. Rename old org `Project` → `Domain`. | 4–5 days |
| **1** | Finish `git-github` `todo!()` stubs. `git-config` SQLite store. `task issue link-forge` + `task issue sync` (pull-only). | 4 days |
| **2** | `git-forgejo` backend. Bidirectional sync via existing `IssueTracker::update_issue`. Per-field conflict resolution. Activity audit log writes (depends on workflows-proto). | 5 days |
| **3** | Webhook receiver (HTTP endpoint on the vox node). Real-time push via `IssueTracker::subscribe`. PR-merged auto-close. | 3 days |

Total: ~3 weeks.

## Acceptance criteria

- [ ] `architect::Entity`-derived `Issue` + sibling entities compile, persist, and round-trip through vault + DB.
- [ ] `Tracker` trait has working `GitHubTracker` + `ForgejoTracker` impls.
- [ ] `task issue sync` pulls all issues from configured repos into the local DB without losing fidelity (round-trip test: pull → push → re-pull gives the same body+labels+assignee).
- [ ] Per-field conflict resolution test: human closes on GitHub while agent edits priority in Task → both wins land in their respective sources.
- [ ] Webhook receiver verifies signatures and applies events idempotently (same payload twice = no-op).
- [ ] All existing `task project *` callers migrated to `task domain *` (the renamed old type).

## Out of scope

- The `workflows-proto` shared primitives (WorkSession, Transition, Activity, Handoff) — see `plans/workflows-orchestrator.md`. Issue's `Activity` log defers to that crate once it lands.
- The `task code` / `task work` CLI surface for the dev loop — see `plans/workflow-coding.md`.
- GitHub Projects (v2 boards) sync. The Issue layer covers issues; project boards are a separate sync surface and can come later.
- GitLab support. The Tracker trait is designed to accommodate it, but the impl is out of scope here.

## File-level breakdown

| File | Change |
|---|---|
| `features/task/task/src/model.rs` | Add `WorkflowAttrs` + `Estimate` + `AgentRef`; add `workflow: Option<WorkflowAttrs>` to `TaskInfo` |
| `features/task/task/src/parse.rs` | Parse `workflow:` frontmatter key |
| `features/task/task/src/write.rs` | Emit `workflow:` frontmatter key |
| `features/workspace/workspace-proto/` | New crate: `Workspace` entity (id, name, default_repos, default_cycle_length, …) |
| `features/project-roadmap/proto/` | New crate: Linear-sense `Project` (target_date, milestones, tracked_progress). Confirm naming with user before locking in. |
| `features/label/label-proto/` | New crate: `Label` (name, color, workspace_id) |
| `features/cycle/` | Audit shape vs Linear sense (length, start_day, rollover); extend if needed |
| `features/git/git-github/src/issues.rs` | Fill `create_issue`, `update_issue`, `list_comments`, `add_comment`, `subscribe` |
| `features/git/git-github/src/reviews.rs` | Fill all `todo!()`s |
| `features/git/git-forgejo/` | New crate. Mirror `git-github` shape over Gitea-compatible REST |
| `features/git/git-config/src/store_sqlite.rs` | SQLite-backed `Store` (alongside existing in-memory) |
| `features/git/git-config/src/sync.rs` | Per-field conflict resolution between `TaskInfo` ↔ forge `Issue` |
| `apps/cli/src/main.rs` | `task issue *` + `task domain *` (renamed) subcommands |
| `apps/server/src/webhook_routes.rs` | New HTTP endpoint for inbound forge webhooks |
| `migration/MIGRATIONS.md` | Document `project → domain` rename + new `workflow:` frontmatter key |
| `org.toml.example` | `[trackers.github]` / `[trackers.forgejo]` examples + `[[trackers.*.repos]]` entries |

## Related

- `plans/workflows-orchestrator.md` — the shared workflow primitives that `Activity` defers to
- `plans/workflow-coding.md` — the `task code` / `task work` CLI built on top of these entities
- `plans/wiki-graph-parity.md` — wiki graph references issues for backlinks; needs the Issue entity to exist
- Memory: `architect-entity-default` — the schema-first rule this plan implements
