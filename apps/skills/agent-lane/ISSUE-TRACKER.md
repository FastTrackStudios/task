# Issue tracker: Task

Issues, maps, workstreams and runs for this repo live in **Task**, not
on GitHub. Use the `task` CLI for all of it.

GitHub Issues remain **human-authored**. Task may cite a GitHub issue;
GitHub never cites Task. This is enforced, not merely agreed: pushing
anything carrying an agent-lane label, or belonging to a workstream,
is refused.

## Conventions

- **Create an issue**: `task issue create "<title>" --body <file|-> [--project <p>]`
- **Read an issue**: `task issue show <id>` (accepts a UUID, an id
  prefix, a vault path, or an exact/unique title)
- **List**: `task issue list`, or `task task list --tag <label>`
- **Ready to work**: `task issue ready`
- **Relations**: `task issue relate <a> blocks <b>`, `task issue relations <id>`
- **Subtasks + rollup**: `task issue subtasks <parent>`
- **Close**: `task issue close <id>`

Every command takes `--org <slug>` and `--server <url>`.

## Triage labels

Four, and no state machine:

| Label | Means |
|---|---|
| `ready-for-agent` | fully specified; a runner may take it |
| `ready-for-human` | needs a person — judgement, access, taste |
| `needs-input` | an agent stopped and asked; resumable |
| `needs-review` | an agent finished green; a branch is waiting |

**Untriaged is the absence of all four.** There is no stored flag, so
nothing can drift out of step with the labels.

Do not conflate `needs-input` with `ready-for-human`: the first is a
parked run you unblock by answering, the second was never agent-able.

## Agent-ready requires a verify command

A ticket an agent may take must resolve to a shell command whose exit
code is the verdict. Resolution order: the ticket's `--verify`, then
its project's `verifyCommand`, then the nearest ancestor project.

A ticket resolving to nothing is **refused** the `ready-for-agent`
label. Prose acceptance criteria are for humans; an agent running
unattended needs an exit code.

## Capabilities

A ticket declares what a runner must have: `records`, `shell`,
`build`, `repo:<owner>/<name>`. `--cap` is repeatable on
`issue create`. A token outside that set is refused.

`build` is separate from `shell` on purpose — a runner may read source
without being allowed to compile.

## Wayfinding operations

- **Map**: an ordinary issue whose body carries the five sections —
  Destination, Notes, Decisions so far, Not yet specified, Out of
  scope. Its tickets are subtasks (`--parent <map>`).
- **Workstream**: what upstream calls a *decision ticket*. Attach with
  `--workstream <id>`. It owns one branch that its tickets' branches
  merge into.
- **Blocking**: `--blocker <id>` on create, or
  `task issue relate <a> blocks <b>`. A ticket is unblocked when every
  blocker is closed.
- **Frontier**: `task issue ready` — open, unblocked, unclaimed. For a
  runner's view of it, `task runner takeable`.
- **Claim**: `task issue start <id> --as-agent <name>` (atomic; two
  agents racing cannot both win).
- **Resolve**: post the answer with `task issue comment`, then
  `task issue close <id>`, then append one line to the map's
  *Decisions so far*.

## Running work

- `task runner register --cap … --max-concurrent N` — register this
  machine.
- `task runner takeable [--why]` — what this runner may take, and why
  it is refusing the rest.
- `task runner work` / `task runner serve` — take one ticket, or run
  continuously.
- `task runner workstream <id>` — work a whole workstream onto one
  branch.
- `task runner surface [--project p]` — everything blocking a human.
- `task runner questions` / `task runner answer <q> <choice>` — the
  grill queue.
- `task runner runs [--ticket t]` — attempt history.
- `task runner watch [run]` — live output.

## When a skill says "publish to the issue tracker"

`task issue create`, with `--parent` when it belongs to a map.

## When a skill says "fetch the relevant ticket"

`task issue show <id>`.

## Pull requests as a triage surface

**No.** External PRs are not a request surface here.
