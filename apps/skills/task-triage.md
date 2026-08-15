---
name: task-triage
description: File unfiled tasks — the ones with no project, parent, workstream, or @context — so they rejoin the working list. Use whenever the untriaged count is non-zero, or on a schedule as a Hermes routine.
runs_as: any agent with the Task MCP surface, or a shell with the task CLI
trigger: self-scheduled (Hermes routine) or on-demand
---

# Task triage

A task that reads only `Telemetry + Observability: Sentry` tells the
user what to do and nothing about why, for whom, or under which
effort. Task hides those from the "Relevant" view on purpose — they're
not an answer to "what should I do now", they're an answer to "what
haven't I sorted". Your job is to drive that count to zero by giving
each one a home.

**The bar is confidence, not coverage.** An unfiled task is visible in
the triage strip and recoverable in one click. A task filed under the
wrong project is *invisible* — it sinks into a queue the user isn't
reading. Leaving ten tasks unfiled costs the user a minute. Filing one
task wrongly can cost them a deadline. When the answer isn't clear,
skip it and say why.

## The triage loop

1. **`list_untriaged_tasks`** returns the unfiled open tasks, oldest
   first, each with its title, body, and age — plus the org's active
   projects and its already-filed tasks, so you have every candidate
   home in the same response. One call, then think. Don't call
   `list_projects` or `list_tasks` again; it's already there.

2. For each task, decide one of:

   **(a) file it.** You have a specific anchor and a reason you could
   defend to the user:

   ```
   file_task { id, project: <uuid>, reason: "<one line>" }
   file_task { id, parent: <task uuid>, reason: "…" }
   file_task { id, workstream: <uuid>, reason: "…" }
   file_task { id, contexts: ["@errands"], reason: "…" }
   ```

   Pass **one** anchor unless two are genuinely both true (a subtask
   of a parent that also belongs to a project). One correct anchor
   beats three guesses.

   **(b) leave it.** No project fits, the title is too thin to
   classify, or it could plausibly go two places. Say so in your
   summary and move on. This is a success, not a failure.

3. Stop when `list_untriaged_tasks` returns `total_untriaged: 0`, or
   when everything left is a deliberate skip. Report what you filed
   and what you didn't, with reasons for both.

## From a shell instead

Same loop, same domain rules, when you have the CLI rather than the
MCP surface (unlike most skills here, this one has both):

```
task task list --untriaged --json          # the queue
task project list --json                   # candidate homes
task task set-project <task-id> <project>  # file it
```

`--untriaged` is the exact complement of `--relevant`: between them
they partition the open set, so a task is in one list or the other,
never neither. `set-project` takes an id or a path. There is no CLI
verb for filing under a parent or workstream yet — use
`task issue` for those, or the MCP `file_task`.

## Decision heuristics

Ordered by how much they justify a write:

- **The body names it.** The task's `details` mention a project by
  name, a repo, a client, or a person tied to one → file there. This
  is the strongest signal; check the body before the title.
- **The title carries a prefix.** `Auth: attach session identity` /
  `Telemetry + Observability: Sentry` — the part before the colon is
  usually the effort. Match it against project titles *and* against
  the titles in `filed_tasks`, which is where an epic would be. If it
  matches a filed task rather than a project, that filed task is the
  parent.
- **A sibling was already filed.** Several tasks sharing a title
  prefix where one already has a home → the rest almost certainly
  share it. Check `filed_tasks` for the prefix before deciding
  nothing fits.
- **It's an errand, not project work.** "Renew the domain", "call the
  bank" — no project will ever fit. Give it `contexts` (`@phone`,
  `@errands`, `@studio`) instead. A GTD context counts as filed, and
  it's the honest answer for standalone work.
- **Domain vocabulary.** A task using a project's distinctive nouns
  (a plugin name, a service name, a song title) belongs to it even
  when it never names the project.

Do **not** file on:

- a generic word overlap ("update", "fix", "the app");
- the project being the user's most active one — recency is not
  membership;
- a guess you'd hedge if asked. Skip it instead.

## Scheduling this as a routine

The Tasks UI's routines panel (agent panel → Routines) creates Hermes
gateway cron jobs. Use the **"Triage unfiled tasks"** preset, or
compose one with this prompt:

```
Run the task-triage skill for this org. Call list_untriaged_tasks,
file what you can place confidently with file_task, and leave the
rest. Reply with one line per task: filed (where + why) or skipped
(why not). If nothing is untriaged, reply "nothing to triage" and
stop.
```

Daily is the right cadence — hourly burns tokens on an empty queue,
weekly lets the strip grow past the point anyone reads it. `0 8 * * *`
lands it before the user opens the board.

Keep each pass cheap: the default `limit` of 50 is plenty, and the
loop is idempotent, so a pass that dies halfway costs nothing.

## Error handling

- `no project with id ...` — you passed a title or a stale id. The
  ids are in the `projects` array of the same `list_untriaged_tasks`
  response; don't reconstruct them.
- `no task with id ...` on `parent` — the parent must be a real task.
  Parents come from `filed_tasks` in the triage response.
- `nothing to file by` — you called `file_task` with only an `id`.
  That's the tool refusing a no-op write; decide an anchor or skip
  the task.
- `a task cannot be its own parent` — you matched a task to itself by
  title. Compare ids, not titles.

## Audit trail

Every `file_task` writes through the normal `TaskService::update`, so
the change lands in the task's markdown page (`projects:` wikilink
included) and in the vault's git history like any human edit. The
`reason` you pass comes back in the response and belongs in your
summary — the user should be able to read what you did and why
without opening a single task.
