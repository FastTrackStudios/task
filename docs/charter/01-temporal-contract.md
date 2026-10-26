# The Temporal Contract

_Charter #1 (formerly issue #1)._

The cross-cutting promise Task makes about **time**: what a date means, what "now" means, how recurrence is expressed, how the past is remembered and the future projected.

Every other charter depends on this one. If time is modelled inconsistently, scheduling, time tracking, recall, and finance all inherit the inconsistency.

## Hard requirements

_To be detailed._

Prompts worth answering: What is the canonical representation — instants, local dates, or both? Whose timezone owns an event? What does a task "due today" mean at 01:00 in another zone? Is recurrence a rule, a materialised series, or both? What happens to a recurring series when one instance is edited? Can the past be rewritten, and if so what is the audit trail?

## Non-goals

_To be detailed._

## Current state

Events carry `start` + `end` in vault frontmatter. Habits/recurrence landed via `worktree/task`. The MCP layer resolves relative dates against a supplied "now" before writing, so absolute ISO dates reach the vault.

## How we evaluate

Each requirement gets a yes/no against current behaviour, re-checked as the system evolves.
