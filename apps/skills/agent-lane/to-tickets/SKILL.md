---
name: to-tickets
description: Break a plan, spec, or the current conversation into tracer-bullet tickets in Task, each declaring its blocking edges, its verify command, and the capabilities a runner needs to take it.
disable-model-invocation: true
---

Break the work into **tickets** — tracer-bullet vertical slices, each
declaring what blocks it.

Read [../ISSUE-TRACKER.md](../ISSUE-TRACKER.md) first.

## Process

### 1. Gather context

Work from what is already in the conversation. If given a reference,
fetch it: `task issue show <id>`.

### 2. Explore the codebase

Use the project's vocabulary in titles and bodies, and respect ADRs in
the area. Look for prefactoring that makes the change easy — "make the
change easy, then make the easy change."

### 3. Draft vertical slices

- Each slice cuts a narrow but **complete** path through every layer.
  Vertical, not a horizontal slice of one layer.
- A completed slice is demoable or verifiable on its own.
- Each fits one fresh context window.
- Prefactoring goes first.

**Wide refactors are the exception.** One mechanical change whose
blast radius fans across the tree cannot land green as a tracer
bullet. Sequence it expand → migrate in batches → contract, each batch
its own ticket blocked by the expand, so CI stays green batch to batch.

### 4. Give every ticket a verdict

This is the part upstream does not have, and it is not optional: a
ticket an agent may take **must resolve to a verify command**, or
nobody can tell when it is done.

Most tickets inherit their project's `verifyCommand` and need nothing.
Set `--verify` only when a ticket needs a narrower or wider check.

Declare capabilities with `--cap` when the ticket needs more than
records: `build` for anything that compiles, `repo:<owner>/<name>` for
a ticket that needs a clone. A ticket that needs none is takeable by
any runner.

### 5. Quiz the user

Present the breakdown as a numbered list — title, blocked by, what it
delivers. Ask:

- Does the granularity feel right?
- Are the blocking edges real, or just a guessed order?
- Should any be merged or split?

Iterate until approved. Do not publish before this.

### 6. Publish

In dependency order, so blockers exist before the tickets naming them:

```bash
task issue create "<title>" \
  --parent <spec-or-map> \
  --project <project> \
  --tag ready-for-agent \
  --estimate <xs|s|m|l|xl> \
  --blocker <id> \
  --cap build \
  --body - <<'EOF'
## What to build

<the end-to-end behaviour this makes work, from the user's
perspective — not a layer-by-layer implementation list>

## Acceptance criteria

- [ ] …

## Blocked by

- <title of each blocker, or "None — can start immediately">
EOF
```

Avoid file paths and code snippets; they go stale fast. The exception
is a snippet from a prototype that encodes a decision more precisely
than prose — a state machine, a schema, a type shape. Trim it to the
decision-rich part.

Do not close or modify the parent.

### 7. Check what you published

```bash
task issue subtasks <parent>
```

The rollup should show your blocked count. If everything is unblocked,
your edges did not land — that is the common failure, and it lets
agents take work out of order.
