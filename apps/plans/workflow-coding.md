# workflow-coding — the agent dev loop (`task code *` / `task work *`)

**Status:** partially shipped — needs triage (2026-07-27). The `task code` command group exists in `apps/task/cli`; how much of the loop this doc describes is wired was not verified.

**Target:** ship the concrete coding workflow — the loop an agent (or human) follows from "what should I work on?" through commit / push / PR / review / merge / close. Replaces `beads`. Primary CLI is `task code *`; `task work *` is a stable alias for ergonomic typing.

Depends on `plans/issue-tracker-integration.md` (TaskInfo + `WorkflowAttrs` + completed `git-{proto,github,forgejo,config}` stack) and `plans/workflows-orchestrator.md` (Workflow trait + Orchestrator runtime).

**Note:** the canonical "issue" in Task is `TaskInfo` (extended with `WorkflowAttrs`) — not a separate `Issue` entity. Throughout this plan, `IssueId` references the `task_proto::TaskId` newtype and the forge-side issue (when one exists) is reached via `git_config::IssueLink`.

## What we have today

- `beads` CLI for local task tracking. Dolt-backed sync. Per-issue claim/close. Hooked into the session-start protocol.
- `git` + `gh` CLIs for repo work. Used ad-hoc by agents.
- No structured per-issue branch / PR / commit linking. No agent-attributed commit trailers. No handoff/resume primitive.

## Design

### Layout

```
features/workflows/coding/
  coding-proto/        # entities + state machine + Workflow impl
  coding-cli/          # task code * (+ task work * alias)
```

### State machine (`coding-proto/src/state.rs`)

```rust
pub enum CodingState {
    Ready,           // claimed; branch not yet created
    Branched,        // branch exists locally
    Committing,      // 1+ commits on branch
    Pushed,          // pushed to remote; no PR yet
    PrDraft,         // draft PR open
    PrOpen,          // PR open for review
    InReview,        // review requested
    ChangesRequested,
    Approved,
    Merged,
    Closed,          // closed without merge
    Parked,          // mid-work, handoff posted
    Blocked,         // can't proceed; surfaced in triage
}
```

Transitions enforced by `CodingWorkflow::transition`. Every state change writes a `Transition` + `Activity` via the orchestrator.

### `CodingWorkflow` (`coding-proto/src/workflow.rs`)

```rust
pub struct CodingWorkflow {
    repos: RepoRegistry,
    issues: Arc<dyn IssueService>,
    tracker: Arc<dyn Tracker>,
    git: GitOps,
}

#[async_trait]
impl Workflow for CodingWorkflow {
    type Subject = IssueId;
    type State = CodingState;
    // ... ready / start / transition / record / park / resume / finish
}
```

### CLI verbs (`coding-cli/src/cli.rs`)

```
task code ready                       [--workspace W] [--limit N]
task code start    <issue-id>         [--branch <name>] [--as-agent <name>]
task code commit   -m "..."           [--no-verify] [--amend]
task code push                        [--draft] [--ready]
task code review                      [--reviewer R] [--auto]
task code merge                       [--squash|--rebase|--merge] [--delete-branch]
task code finish   <issue-id>         [--close-issue]
task code resume   <issue-id>         [--as-agent <name>]
task code park     "<reason>"         [--needs-input "..."]
task code blocked  <issue-id> "<why>"
task code inbox                       [--as-agent <name>]
task code status                                          # current branch + issue + PR + last activity
task code link-pr  <issue-id> <pr-url>                    # attach an existing PR
task code import-beads                                    # one-time migration
```

`task work <verb>` is a hard alias for `task code <verb>` registered in the same clap command tree.

### What each verb does

#### `task code ready`

```rust
async fn cmd_ready(workspace: WorkspaceId, agent: AgentRef) {
    let candidates = issues
        .list(filter!(workspace, status in [Triage, Backlog, Todo], assignee = None | agent))
        .await?;
    let filtered = candidates.filter(|i| !i.has_unresolved_blockers()).take(N);
    print_table(filtered);
}
```

Surfaces issues with: no blockers, in the agent's workspaces, matching priority preferences. Open handoffs surface first (something is partially done).

#### `task code start <issue-id>`

1. Verify issue exists + agent has `Capability::Claim`.
2. `git switch -c <prefix>/<issue-id>-<slug>` (configurable prefix per workspace, default `feat/`).
3. `Orchestrator.execute(Intent::Start, agent)` — opens a `WorkSession`, transitions Issue → InProgress, transitions CodingState → Branched.
4. Records `Activity::Started { branch, sha }`.

#### `task code commit -m "..."`

Wraps `git commit` and injects two trailers:

```
fix(parser): handle nested quotes in YAML scalars

Co-Authored-By: Claude <noreply@anthropic.com>
Task-Agent: claude-opus-4-7
Task-Issue: PROJ-142
Task-Session: 01JFXG3K…
```

Records `Activity::Commit { sha, message }`.

#### `task code push`

1. `git push -u origin <branch>`.
2. If no PR exists: `Tracker::open_pr(repo, issue, branch, draft=true)`.
3. Wires the issue ↔ PR link: writes `Closes #N` (or Forgejo equivalent) in the PR body if not present.
4. Records `Activity::Pushed { branch }` + `Activity::PrOpened { url }` if applicable.

#### `task code review`

1. `Tracker::mark_ready_for_review(pr)` (un-draft).
2. Optionally `Tracker::request_reviewers(pr, [reviewers])`.
3. With `--auto`: invoke the local `review` skill (Claude Code review subagent) and post its findings as a PR comment.

#### `task code merge`

1. Verify PR has required approvals + green CI (configurable per workspace).
2. `Tracker::merge_pr(pr, strategy)`.
3. On merge success: transition Issue → Done, close session, optionally `git branch -d`.

#### `task code resume <issue-id>`

Calls `Orchestrator.resume`:

```
=== Resuming PROJ-142 (session 01JFXG3K…) ===
Branch:    feat/proj-142-yaml-nested-quotes  (3 commits ahead of main)
PR:        FastTrackStudios/task#891 (draft, CI: green)
State:     ChangesRequested (paused 4h ago)
Last activity:
  - commit a3f2e9b "test: add nested-quote fixtures" by claude-opus-4-7
  - review-request from cody, requested changes: "split parser logic out"
Open handoff:
  > Reviewer asked to extract parse_quoted() into its own module.
  > Done: identified call sites in scanner.rs + lexer.rs
  > Open question: should the new module live under parser/ or a new quoting/?
Related (via wiki-graph):
  - wiki/concepts/avoid-intermediate-collections.md
  - features/yaml-parser/src/scanner.rs  (touched 3 times last session)
```

The agent sees the world the previous agent left behind, no re-grep required.

#### `task code park "<reason>"`

Creates a `Handoff` with `reason: HandoffReason`, summary (auto-drafted from recent activity, agent can edit), open_questions + recommended_next prompted as inputs. Returns the agent to triage.

#### `task code inbox --as-agent <name>`

Combined queue:

1. Open `Handoff`s the agent is `to:` of (explicit assignments).
2. Open `Handoff`s with `to: None` (anyone-can-claim) matching the agent's capabilities.
3. `Issue`s in `Triage` if the agent has `Capability::Triage`.
4. `Issue`s in `InProgress` claimed by this agent (resume candidates).

Sorted: handoffs first, then by priority + age.

### Commit-trailer attribution + git-log audit

Every commit's `Task-Agent` + `Task-Issue` + `Task-Session` trailer means:

- `git log --grep "Task-Issue: PROJ-142"` recovers every commit for an issue across all repos.
- `git log --grep "Task-Agent: claude-opus-4-7"` recovers everything that agent touched.
- The DB activity log can be reconstructed from git-log if the DB is lost — git becomes the canonical record.

### PR-merge → auto-close issue

The webhook receiver from `plans/issue-tracker-integration.md` Phase 3 catches the `pull_request.closed` event. If the PR's `Closes #N` references an issue, `CodingWorkflow.transition(session, Merged)` runs, which transitions Issue → Done and closes the WorkSession.

### Capability defaults

```toml
[[agents]]
name = "triage-bot"
workflows = ["coding"]
allow = ["Read", "Transition(Triage→Backlog)", "Comment"]

[[agents]]
name = "claude-opus-4-7"
workflows = ["coding"]
allow = ["*"]   # full access

[[agents]]
name = "review-bot"
workflows = ["coding"]
allow = ["Read", "Comment", "Capability::Review"]
```

### Beads migration

`task code import-beads`:

1. Reads the local Beads sqlite (path from `.beads/` config).
2. For each open issue: creates an `Issue` with status mapped (`open` → `Todo`, `in_progress` → `InProgress`, etc.). Sets `source: Local` (Beads issues aren't on GitHub by default).
3. For each dependency: creates an `IssueLink::Blocks`.
4. For each `bd memories` entry: ... already in auto-memory; no migration needed.
5. Marks the `.beads/` dir as deprecated (rename to `.beads.archived/`); updates the session-start hooks to call `task code ready` instead of `bd ready`.

Acceptance: `task code ready` shows the same issues `bd ready` did before migration.

### Phases

| Phase | Scope | Effort |
|---|---|---|
| **0** | `CodingState` + `CodingWorkflow` impl. CLI scaffold (`code` + `work` alias). Local-only (no remote sync). | 3 days |
| **1** | `task code start / commit / push` with branch + commit-trailer attribution. Auto-PR-on-push. | 3 days |
| **2** | `task code review / merge` flows. Webhook auto-close. CI status integration. | 4 days |
| **3** | `task code resume / park` with full `ResumeContext`. Handoff drafting prompts. | 3 days |
| **4** | `task code inbox`. Capability enforcement. | 2 days |
| **5** | Beads import + deprecation. Hook rewrites. | 1 day |

Total: ~16 days (~3 weeks).

## Acceptance criteria

- [ ] `task code start → commit → push → review → merge → finish` round-trip works on a real GitHub repo (FastTrackStudios/task) with all transitions recorded as `Transition` + `Activity` rows.
- [ ] Commit trailers (`Task-Agent`, `Task-Issue`, `Task-Session`) appear on every commit made via `task code commit`.
- [ ] Webhook receives `pull_request.closed (merged=true)` and auto-transitions the linked Issue → Done.
- [ ] `task code resume` after `task code park` returns full `ResumeContext` with last 10 activities, open handoff, related refs.
- [ ] `task code inbox --as-agent claude-opus-4-7` returns handoffs + claimed-in-progress + triage candidates, sorted correctly.
- [ ] Capability check: `review-bot` calling `task code merge` returns `Forbidden`.
- [ ] `task work` alias produces identical output to `task code` for every verb.
- [ ] `task code import-beads` migrates all open issues and dependencies; `task code ready` post-migration surfaces the same set `bd ready` would have.
- [ ] `.beads.archived/` exists after migration; session-start hooks updated to call `task code ready`.

## Out of scope

- Web UI for the coding workflow. CLI + RPC only.
- GitLab support (Tracker trait accepts it; impl deferred).
- Multi-repo atomic operations (a single issue spanning commits in two repos). Phase-1 model: one issue → one branch → one repo → one PR. Cross-repo coordination via Issue links + multiple sessions.
- Automated CI failure triage. The workflow surfaces CI status but doesn't drive re-run / retry.
- Stacked PRs / git-spice / graphite-style branch stacks. Phase-1: one branch per issue.

## File-level breakdown

| File | Change |
|---|---|
| `features/workflows/coding/coding-proto/Cargo.toml` | New crate |
| `features/workflows/coding/coding-proto/src/state.rs` | `CodingState` enum + transition table |
| `features/workflows/coding/coding-proto/src/workflow.rs` | `CodingWorkflow` impl of `Workflow` trait |
| `features/workflows/coding/coding-proto/src/trailer.rs` | Commit-trailer formatter + parser |
| `features/workflows/coding/coding-cli/Cargo.toml` | New crate |
| `features/workflows/coding/coding-cli/src/cli.rs` | clap subcommand tree + `work` alias |
| `features/workflows/coding/coding-cli/src/cmd/*.rs` | One file per verb (start, commit, push, review, merge, resume, park, inbox, …) |
| `features/workflows/coding/coding-cli/src/import_beads.rs` | One-time migration command |
| `apps/cli/src/main.rs` | Wire `task code` + `task work` into the top-level CLI |
| `.beads/MIGRATION.md` | One-pager pointing at `task code import-beads` |
| `CLAUDE.md` / session-start hooks | Replace `bd ready` references with `task code ready` |

## Related

- `plans/issue-tracker-integration.md` — Issue entity + Tracker trait this workflow consumes
- `plans/workflows-orchestrator.md` — `Workflow` trait + `Orchestrator` this workflow plugs into
- `Knowledge/wiki/concepts/wiki-graph.md` — `ResumeContext.related` queries the graph
- Memory: `architect-entity-default` — the schema-first rule
- Skill: `review` — invoked by `task code review --auto`
