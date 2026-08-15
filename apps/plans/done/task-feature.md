# Task feature — first-party tasks

Status: foundation shipped (`features/task/task/`), CLI live
(`task task capture / list / done`). Forward-looking work below.

## Direction

First-party integration of the `callumalpass/tasknotes` model:
each task is a markdown page with YAML frontmatter; views are
`.base` files. Schema mirrors TaskNotes verbatim so existing
TaskNotes vaults round-trip without conversion.

Why: it's a real-world product with thousands of users, the
on-disk format is already battle-tested, and our existing
`vault-live::bases` parser already evaluates the `.base` query
DSL TaskNotes uses for every view.

Research notes captured in `~/.claude/.../memory/reference_tasknotes_deep.md`.

## Shipped (slice 1)

- `features/task/task/` — foundation crate.
  - `TaskInfo` mirrors TaskNotes' `TaskInfo` interface
    (`src/types.ts:453`): same field names (`due`, `scheduled`,
    `timeEstimate`, `timeEntries`, `recurrence`,
    `complete_instances`, etc.).
  - `parse_page` / `scan_vault` / `serialize_task` / `write_task`
    — round-trip clean. Discriminator: `type: task` field OR
    `task` in the `tags` array.
  - `capture(input)` — minimal NLP: `#tag` / `@context` /
    `[[Project]]` / `!priority` / date keywords (`today`,
    `tomorrow`, `next monday`, bare weekday, `YYYY-MM-DD`).
  - `default_task_path(title, folder)` — slug-based:
    `Buy Milk!` → `tasks/buy-milk.md`.

- `apps/cli` — three subcommands under `task task`:
  - `capture "<text>"` — creates `<vault>/tasks/<slug>.md`.
  - `list [--status] [--tag] [--context]` — scan + print.
    Sort: open before done, then due-asc (None last), then
    title.
  - `done <task_id> [--undo]` — flip `status: done` + set
    `completedDate`.

End-to-end against `examples/vault/`:

```
$ task task capture "Buy milk tomorrow #errands @shopping !high"
Created examples/vault/tasks/buy-milk.md
$ task task list --status open
[ ] Buy milk ! (due 2026-05-22)    tasks/buy-milk.md
```

7 unit tests pass.

## Open follow-ups

### Slice 2 — Time tracking
Schema already records `timeEntries: [{ startTime, endTime }]`.
CLI needs `start <task_id>` (append a row with `startTime`,
no `endTime`) + `stop` (close the open row on the currently-
running task) + `time-status` (which task, how long).
Append-only, single-task-running-at-a-time invariant.

### Slice 3 — `.base` view runner
`vault-live::bases::parse` already produces a `ParsedBase`.
`vault-live::bases::execute_view` already evaluates filters
over rows. The work: feed the TaskInfo set as rows + evaluate
formulas (priorityWeight, daysUntilDue, urgencyScore, etc.).
TaskNotes ships ~40 formulas in its default `.base` — most
straightforward, a few date-arithmetic-heavy.

Once shipped, `task task list --view tasks-default.base` runs
the same query an Obsidian TaskNotes installation would.

### Slice 4 — Kanban / calendar UI
Lives in `features/task/task-ui/` (not yet created). Dioxus
components: `<TaskList>`, `<Kanban groupBy="status">`,
`<Calendar>`. Reads the same `.base` view output as the CLI
runner. Probably also the home for inline-edit (toggle status
inside the kanban card → write back via `task::write_task`).

### Slice 5 — Recurrence engine
Schema has `recurrence` (RRULE) + `complete_instances[]` +
`recurrence_anchor` (`"scheduled" | "completion"`). Need a
next-occurrence calculator + a "mark instance complete" CLI
verb. Probably reach for the existing `rrule` workspace dep.

### Slice 6 — Configurable field mapping
TaskNotes' `FieldMapper` lets users rename `due` → `deadline`
etc. v1 of our schema is hard-wired to TaskNotes defaults;
mapping comes later. Likely a `~/.config/task/fields.toml` or
in-vault `.task/fields.toml`.

### Slice 7 — Server-mountable TaskService
If a desktop client wants to ask a remote server "list all
my tasks" without pulling the whole vault, expose
`task::scan_vault` as a `#[architect::rpc] trait TaskService`
that returns parsed `TaskInfo`s over vox. Today the CLI runs
locally against the vault root; only matters once the
multi-server desktop work (`vault-sync-desktop-multiserver`)
lands.

## Deliberately NOT in the roadmap

- Obsidian-plugin specifics (Bases UI integration, Pomodoro
  view, Google/Microsoft calendar OAuth).
- HTTP API surface — architect-rpc over vox is the better fit.
- Per-task CRDT collaboration — the editor handles per-file
  collab later; tasks-as-files don't need it.
