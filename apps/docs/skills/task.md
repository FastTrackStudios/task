---
name: task
description: Drive Task's issue + git workflow from the CLI — the bd/Linear replacement. Find work, atomically claim it, triage a PRD-issue into agent-sized subtasks, run the git loop (branch/commit/push/PR), park/resume for handoff, and sync to GitHub/Forgejo. Use whenever an agent (or human) is picking up, tracking, or shipping coding work in a repo that has a Task org. Multiple agents run this in parallel via worktrees.
---

# Task — the agent working loop

Task is the issue tracker + agent dev loop (replaces beads). An issue is a **PRD**: the what + acceptance criteria in its body. You take an issue, triage it into agent-sized subtasks, claim one atomically, and ship it through git. Parallel agents each work a subtask in their own worktree.

`task <verb>` is the CLI. It talks to a running `task-server` over vox; set `TASK_VOX_URL` (e.g. `ws://127.0.0.1:18080/vox`) and pick the org with `--org <slug>` (or `task auth use <slug>`). Forge sync needs `TASK_FORGEJO_TOKEN` / `TASK_GITHUB_TOKEN` + `TASK_FORGEJO_BASE_URL` in `.env`.

## The model

```
Org → Project → Issue (PRD) → Subtasks → commits/PRs
                  │
   Cycle / Milestone / Goal are the time + target axes.
```

- **Issue** = a `TaskInfo` (markdown in the vault). Carries Linear-style `workflow:` attrs (cycle, estimate, assignees=the *claim*, blockers, parent, session).
- **Subtask** = a TaskInfo with `workflow.parent` = the issue. The issue is done when all subtasks close.
- **Claim** = `workflow.assignees`. Atomic, server-enforced — two agents can't hold the same task.

## The loop (one agent)

```bash
# 1. Find work — open, unblocked, claimable by me
task issue ready --as-agent claude@opus-4-7

# 2. (if the issue is a PRD that needs breaking down) triage it
task issue triage <issue-id> --subtask "wire schema" --subtask "impl handler" --subtask "tests"
#   …or pipe an LLM-generated breakdown:  echo "$breakdown" | task issue triage <id> --from -

# 3. Claim atomically (fails if someone else holds it)
task issue claim <subtask-id> --as-agent claude@opus-4-7

# 4. Start the git loop — branch + claim + in-progress
task code start <subtask-id> --as-agent claude@opus-4-7            # in current checkout
task code start <subtask-id> --as-agent claude@opus-4-7 --worktree # isolated worktree (parallel)

# 5. Work, then commit with attribution trailers (Task-Id/Task-Agent)
task code commit -m "fix: handle nested quotes" --all

# 6. Push + open a PR that closes the linked forge issue on merge
task code push                 # injects "Closes #N", links the PR to the task

# 7. Review/merge — when green, merge; the forge auto-closes the issue
task issue pr-merge --repo OWNER/REPO <pr-number> --method squash [--close-task <id>]

# 8. Reflect local state from the forge
task issue sync --repo OWNER/REPO          # one repo
task issue sync-all                        # every linked repo (cron-friendly)
```

`task work *` is an alias for `task issue *`.

## Parallel agents on one issue

This is the point. Triage into N subtasks, then N agents each grab one and work it **in their own git worktree** (separate dir, shared object store + build cache, own branch) — no collisions:

```bash
task issue triage <issue> --subtask A --subtask B --subtask C
# agent 1:
task code start <A> --as-agent claude --worktree   # → <repo>-wt-<A>… (sibling dir)
# agent 2 (concurrently):
task code start <B> --as-agent codex  --worktree
# each: cd into its worktree, code commit / code push → its own PR

task code worktrees        # live map of who's where
task code cleanup <id>     # remove a worktree after its PR merges
```

Worktrees are created as **siblings** of the repo (so the workspace's `../` path-deps resolve) and share the main `target/` (the git hooks set `CARGO_TARGET_DIR`), so builds in a worktree are fast + correct.

## Handoff (park / resume)

When you must stop mid-subtask, hand it off with context instead of dropping it:

```bash
task code park "Scaffold's in; stuck on the auth header format." \
  --reason needs-input --open "Which header — token or Bearer?" --as-agent claude
# → records a handoff, releases the claim, returns the task to the ready queue

# another agent:
task code inbox --as-agent codex          # see parked work
task code resume <id> --as-agent codex    # claim + print the handoff + switch to the branch
```

## Triage discipline (the PRD → subtasks step)

- An issue body should read like a small PRD: goal, acceptance criteria, constraints.
- Triage **when it's time to start it**, not before. Subtasks should be agent-sized (one focused PR each), independently claimable, and ideally not block each other so they can run in parallel.
- Use `blockers` only for true ordering (`task issue set-workflow <id> --add-blocker <other>`); use `parent` for "part of."

## Inspect / triage views

```bash
task issue list [--status open] [--project <uuid>] [--has-workflow]
task issue show <id>
task issue subtasks <issue-id>      # board: each subtask's status + claimant + done/total
task issue stats                    # counts by status/priority/project/assignee
task project progress <project-id>  # rollup % from the project's tasks
task label create bug --color d73a4a --group type     # org-scoped labels
```

## Forge setup (once per repo)

```bash
# If you're repo-admin: register a webhook so closes propagate instantly.
task setup forge --repo OWNER/REPO [--github] --public-base https://tasks.host

# Either way (incl. third-party repos where you can't install a webhook):
task issue push <id> --repo OWNER/REPO [--github]    # local task → forge issue
task issue pull --repo OWNER/REPO [--github]         # forge issues → local
# then `task issue sync` / `sync-all` keeps state reconciled on demand.
```

GitHub vs Forgejo is the `--github` flag (or inferred from the git remote in `task code`). Same verbs for both.

## Conventions

- Agent ref: `agent:<name>[@<model>]` (e.g. `agent:claude@opus-4-7`) or `human:<user-id>`.
- Commit trailers (`task code commit` adds them): `Task-Id`, `Task-Agent`, `Co-Authored-By`. `git log --grep "Task-Id: <short>"` recovers every commit for a task.
- Don't hand-merge from the forge UI mid-loop if you can `task issue pr-merge` — it keeps local state in step.
- One subtask → one branch → one PR. Keep them small.

## One-line lifecycle

```
create (PRD) → triage → claim → code start[--worktree] → commit → push (PR Closes #N)
   → review → merge → forge auto-close → sync → subtask done → parent done
```
