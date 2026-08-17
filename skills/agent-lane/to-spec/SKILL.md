---
name: to-spec
description: Turn the current conversation into a spec and publish it to Task — no interview, just synthesis of what you have already discussed.
disable-model-invocation: true
---

Take the conversation and produce a spec. **Do not interview** — you
already know what was said.

Read [../ISSUE-TRACKER.md](../ISSUE-TRACKER.md) first.

## Process

1. **Explore the repo** if you have not already. Use the project's
   vocabulary throughout, and respect ADRs in the area.

2. **Sketch the seams** you will test the feature at. Prefer existing
   seams. Use the highest one that works. Fewer is better — one is
   ideal. **Check the seams with the user before writing the spec**:
   getting this wrong is expensive, and it is the one thing here worth
   a round-trip.

3. **Write and publish**:

```bash
task issue create "<title>" --project <p> --tag ready-for-agent \
  --priority high --estimate xl --body ./spec.md
```

<spec-template>

## Problem Statement

The problem, from the user's perspective.

## Solution

The solution, from the user's perspective.

## User Stories

A long, numbered list: *As an `<actor>`, I want `<feature>`, so that
`<benefit>`.* Cover every aspect — this is where scope becomes
concrete, and a thin list here produces a spec that argues later.

## Implementation Decisions

Modules built or modified, their interfaces, architectural decisions,
schema changes, API contracts, specific interactions.

No file paths, no code snippets — they go stale. The exception is a
snippet from a prototype that encodes a decision more precisely than
prose (a state machine, a reducer, a type shape); inline it, trimmed
to the decision-rich part, and note where it came from.

## Testing Decisions

What makes a good test here (external behaviour, never implementation
detail), which modules are tested, and prior art in the codebase to
follow.

## Out of Scope

What this deliberately does not do.

## Further Notes

Anything else worth knowing.

</spec-template>

4. **Break it down** with `/to-tickets`. A spec nobody sliced is a
   spec nobody starts.
