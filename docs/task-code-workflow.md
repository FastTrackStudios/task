# `task code` — the agent dev loop

`task code` wraps git around the issue lifecycle so an agent (or human) drives a task from claim to merged PR with four verbs. The work branch is the only state it needs.

## The loop

```bash
# 1. Claim a task + branch off HEAD (task/<short-id>-<slug>)
task code start <task-id> --as-agent claude@opus-4-7

# 2. Do the work, then commit with attribution trailers
task code commit -m "fix: handle nested quotes" --all

# 3. Push + open a PR that closes the task's linked issue on merge
task code push

# 4. (anytime) see where you are
task code status
```

## What each verb does

| Verb | Action |
|---|---|
| `start <id>` | Resolve the task, flip it to `in-progress`, append the agent to assignees, create branch `task/<short>-<slug>` off HEAD. |
| `commit -m … [--all]` | `git commit` with `Task-Id` (parsed from the branch), `Task-Agent`, and `Co-Authored-By` trailers. `--all` stages first. |
| `push [--base main] [--draft]` | Push the branch; infer the forge repo from `git remote get-url origin`; open a PR injecting `Closes #N` for the task's linked issue; record a PR link on the task. |
| `status` | Show the current branch's task + its linked issue/PR. |

## How it knows what it's working on

- **Task** — the 8-char task-id prefix round-trips through the branch name `task/<short>-<slug>`. No state file.
- **Forge repo** — parsed from the git remote. Handles `forgejo@host:owner/repo.git`, `git@github.com:owner/repo.git`, and `https://…` forms. Works on third-party repos (you only need a token).
- **Issue link** — read from the per-org `issue-links.json` so `push` can inject the right `Closes #N`.

## Commit-trailer attribution

Every `task code commit` stamps the git log itself:

```
fix: handle nested quotes

Task-Id: 49ddc8fd
Task-Agent: claude@opus-4-7
Co-Authored-By: Claude <noreply@anthropic.com>
```

`git log --grep "Task-Id: 49ddc8fd"` recovers every commit for a task across the repo; `--grep "Task-Agent: claude"` recovers everything an agent touched — and it survives even if the issue DB is lost.

## On merge

The PR body's `Closes #N` makes the forge auto-close the linked issue when the PR merges. The webhook receiver (admin repos) or `task issue sync` (any repo) then flips the local task to `done`. The loop closes itself.

## Relationship to the other surfaces

- `task issue *` — the data view (create/list/triage/close, forge sync).
- `task code *` — the git-side dev loop on top of it.
- `task setup forge` — one-time webhook wiring.
