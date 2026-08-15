# Git feature

**Status:** partially shipped — needs triage (2026-07-27). `git-{proto,config,github,forgejo}` all exist under `features/task/git/`; the later git-plumbing phases were not verified.

Issue + PR style workflow on top of forges (GitHub, Forgejo, eventually others). Wire contract first; actual git plumbing (status/diff/commit) lives in a later phase.

## Why

Task already has projects and tasks as markdown pages in the vault. We want to bind a project to a forge repo and a task to an issue or PR, so:

- Closing a task closes its issue (and vice versa via webhook ingest later).
- A project dashboard can show open issues / in-flight PRs alongside its own tasks.
- The agent feature can act on an issue thread the same way it acts on a task.

The forge's own issue tracker stays canonical for cross-team / external collaboration. Task mirrors and links — does not replace.

## Shape (mirrors `email-proto` family)

```
features/git/
  git-proto/         wire contract: three #[architect::rpc] traits + payloads + events
  git-github/        impl via octocrab
  git-forgejo/       impl via forgejo-api (next pass)
  git-config/        RepoBinding + IssueLink persistence
  (later) git-store/ SQLite cache of issues/PRs (matches email-store)
  (later) git-link/  outbox bridging task lifecycle <-> forge updates
```

### Three traits

Split chosen to let backends declare partial support (e.g. a read-only mirror impls `IssueTracker` reads only; a non-PR forge skips `ReviewSurface`):

- `RepoCatalog` — `list_repos`, `get_repo`. Repo-level metadata that bindings depend on.
- `IssueTracker` — issue CRUD, comments, labels, assignees, milestones, state transitions.
- `ReviewSurface` — PR CRUD, review threads, requested reviewers, merge.

Each trait carries its own `subscribe(repo, tx: Tx<GitEvent>)` (or a single shared event stream — TBD; start per-trait, fold later if redundant).

Each gets its own module (`git_proto::repo`, `git_proto::issues`, `git_proto::reviews`) so the auto-emitted `serve` / `Client` / `descriptor` / `Service` / `Dispatcher` names don't collide.

## First-pass scope

`git-proto` + `git-github` only. `git-github` real for `list_issues` + `get_issue`; the rest compile as `todo!()` stubs. `git-config` skeleton with types but no DB wiring. No Forgejo yet — the slot is reserved.

Subsequent passes:
1. Fill out remaining `IssueTracker` methods on `git-github`.
2. `git-forgejo` backend (forgejo-api crate). Mirror the same trait impls.
3. `git-config` SQLite persistence + project/task lookup helpers.
4. `git-store` cache layer behind the trait (so UI doesn't pay a round-trip).
5. `git-link` outbox: task status change -> issue update via the trait.

## Crate inventory (external)

- `octocrab` — modern strongly-typed GitHub API client. Async. Mature handlers for issues + pulls.
- `forgejo-api` — Forgejo's Web API.
- `gitea-sdk` / `gitea-client` — superseded by `forgejo-api` for our use; Forgejo's surface is a Gitea superset.

No unified Rust forge crate exists in the ecosystem — confirms the architect::rpc abstraction is the right call.

## Prior art consulted

CodexMonitor (`research/CodexMonitor/src-tauri/src/shared/git_rpc.rs`) — JSON-RPC method inventory but shells out to `git` and `gh`. Single-provider (GitHub via `gh`). Useful as an **operation checklist**, not an architecture template. We pull the method list (`list_issues`, `get_pr_diff`, `checkout_pr`, etc.) but skip the shell-out design — backends call typed API clients directly.

## Binding model

`git-config` owns two persisted types:

- `RepoBinding { project_id, forge, owner, repo }` — one project can map to N repos, one repo to N projects (many-to-many).
- `IssueLink { task_id, forge, owner, repo, number, kind: Issue | Pull }` — one task can link to N issues/PRs.

`forge` is a typed enum (`Github | Forgejo { base_url }`), so the binding tells the dispatcher which backend serves a given lookup.

## Open questions

- Auth: per-backend token retrieval via `email-secret`-style abstraction, or per-backend config? Likely the former once `git-secret` lands.
- Webhook ingest vs polling: backends decide. `git-github` can poll first, webhook later via the agent server. `git-forgejo` self-hosted can ship a webhook receiver day one.
- Project boards / GitHub Projects v2: out of scope for first pass — that's GraphQL-only and very GitHub-specific. Revisit after the REST surface is stable.
