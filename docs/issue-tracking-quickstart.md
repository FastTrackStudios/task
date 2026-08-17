# Issue tracking — quick start

The minimum workflow to start using Task as your project tracker (beads-equivalent surface, with Linear-style attributes underneath).

## Start the server

The CLI's `task issue *` verbs talk to a running `task-server`:

```bash
# in one terminal
cargo run --bin task-server
```

The server reads orgs from `~/.task/orgs/<slug>/` and serves vox over WebSocket. Default endpoint: `ws://127.0.0.1:9090/vox`. Override with `TASK_VOX_URL` in your `.env`.

## Pick an active org

```bash
task auth use <org-slug>   # e.g. codywright
```

This persists the selection — subsequent `task` commands target that org without `--org`.

## Create an issue

```bash
task issue create "Fix YAML scalar quoting bug" \
  --priority high \
  --estimate s \
  --tag bug --tag parser \
  --body 'Reproduce: ...'
```

Aliases: `task work create ...` does the same thing.

Optional Linear-style attrs (drop into the `workflow:` frontmatter block):

```bash
task issue create "Land mobile launch" \
  --workspace 7f3a2b1e-... \
  --cycle    9a2c4d5f-... \
  --project  4ee1abcd-... \
  --estimate l \
  --assignee agent:claude@opus-4-7 \
  --assignee human:cody \
  --blocker  <task-uuid>
```

## Drive the loop

```bash
task issue ready                        # what should I pick up?
task issue ready --as-agent claude      # what's mine / unowned?
task issue start <id> --as-agent claude # claim + flip to in-progress
# ... do the work ...
task issue close <id>                   # done
```

## Inspect

```bash
task issue list --status open
task issue list --has-workflow                # only the migrated/tracked ones
task issue show <id>
task issue assignees <id>
task issue stats                              # overview by status/priority/workspace/assignee
```

## Adjust workflow attrs on an existing task

```bash
task issue set-workflow <id> \
  --workspace <uuid> \
  --add-assignee agent:claude \
  --add-blocker <other-uuid>

task issue set-workflow <id> --clear         # drop the workflow block (back to plain task)
```

## Naming conventions

| You type | Shipped today | Next |
|---|---|---|
| `task task *` | TaskNotes-shape CRUD (titles, due, scheduled, tags) | unchanged |
| `task issue *` | workflow-aware operations on the same TaskInfo | **+ git integration (PRs, branches) next session** |
| `task work *` | alias for `task issue` — same verbs, shorter typing | unchanged |
| `task code *` | not yet — will wrap `task issue` with git-side operations (branch, commit, push, PR) | upcoming |

## What's not there yet

- **No `task workspace create`** — Workspace entity exists but has no CRUD service wired. You can still reference workspace UUIDs in filters; the UI/CRUD layer lands next.
- **No webhook receiver** — GitHub/Forgejo events don't yet auto-close linked tasks. Manual `task issue close` only.
- **No `git-config` SQLite store** — `task-server` won't yet *persist* the TaskInfo ↔ forge-Issue link across restarts. Will land alongside the webhook receiver.
- **No `task code` CLI** — the agent dev-loop verbs (`start` → branch, `commit`, `push` → PR, `merge`) are scoped for a follow-up session. The orchestrator (`workflows-orchestrator`) is the prerequisite.
- **Beads migration** — `task issue import-beads` not yet built. Manual recreate for now if you have beads issues to bring across.

## Provenance — what shipped this session

Each commit is a stand-alone vertical-ish slice:

```
2e18319 feat(cli): task issue create/ready/start/close + 'task work' alias
14b4b9d feat(cli): task issue * — Linear-style view of TaskInfo
e60f9b6 feat(git-forgejo): Gitea-compatible REST backend for git-proto
3b49d3d feat(workspace/label/project-roadmap): Linear-style container entities
99987bb feat(task): WorkflowAttrs on TaskInfo — Linear-style work attributes
ce5d9de feat(workflows-proto): cross-domain workflow primitives
```

Plus the `task issue stats` follow-up.

## Roadmap

- Issue-tracker integration — data model + Tracker traits + per-field sync
- Workflows orchestrator — `Workflow` trait + Orchestrator runtime + handoff/resume
- Workflow coding — `task code` CLI + commit-trailer attribution + auto-PR
