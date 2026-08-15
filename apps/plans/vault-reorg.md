# Vault top-level reorganization

Status: in progress. Trims `examples/vault/` top level from
~16 folders to 9, with a clearer split between human-authored
notes, project workflow data, instance/asset registry, and
historical records. Crate path constants flip to match.

Out of scope: renaming crates, moving crate source code,
changing wire types. Only string path constants and the
example vault tree.

## Top level — before / after

Before (16):

    Bases/ body/ Daily/ Fleeting/ Inbox/ locations/
    mealplan/ Meetings/ pantry/ People/ Projects/
    scheduling/ Stubs/ tasks/ Wiki/ Wisdom/

After (9 + root note):

    Inbox/        capture; absorbs Fleeting
    People/       personal relationships (or org employees)
                  — NOT public figures
    Wiki/         reference / concepts / transferable knowledge
                  (vault-scoped — institutional reference)
    Wisdom/       atomic zettels (one claim per note)
    Journal/
      Daily/
      Meetings/
    Projects/     active workflow containers
      Mealplan/
        meals/
        mealplan.base
      Fitness/
        body/
      Scheduling/
        event-types/
        schedules/
        templates/
    Task/         task notes (TaskNotes shape) + tasks.base
    Operations/   instance / asset registry
      Locations/
      Inventory/
        Pantry/
    Records/      append-only history
      bookings/
    Welcome.md

## File-by-file mapping (example vault)

| From | To |
|---|---|
| `Daily/` | `Journal/Daily/` |
| `Daily/Daily.md` | `Journal/Daily/Daily.md` |
| `Meetings/` | `Journal/Meetings/` |
| `Meetings/Meetings.md` | `Journal/Meetings/Meetings.md` |
| `Fleeting/*.md` (3 notes) | `Inbox/` |
| `Fleeting/Fleeting.md` | delete |
| `Stubs/` (all) | delete |
| `Bases/All concepts.base` | `Wiki/all-concepts.base` |
| `Bases/Daily Plan blocks.base` | `Journal/Daily/daily-plan-blocks.base` |
| `Bases/Meal plan.base` | `Projects/Mealplan/mealplan.base` |
| `Bases/Open tasks.base` | `Task/open-tasks.base` |
| `Bases/Upcoming bookings.base` | `Records/bookings/upcoming-bookings.base` |
| `Bases/Bases.md` | delete |
| `People/Andrej Karpathy.md` | merge into `Wiki/entities/andrej-karpathy.md` |
| `People/Bret Victor.md` | `Wiki/entities/bret-victor.md` |
| `People/Linus Torvalds.md` | `Wiki/entities/linus-torvalds.md` |
| `People/Marijn Haverbeke.md` | `Wiki/entities/marijn-haverbeke.md` |
| `People/People.md` | stays; reframed for relationships |
| `tasks/` | `Task/` |
| `tasks/tasks.md` | `Task/Task.md` |
| `mealplan/*.md` | `Projects/Mealplan/meals/` |
| `pantry/` (none yet on disk) | `Operations/Inventory/Pantry/` |
| `scheduling/event-types/` | `Projects/Scheduling/event-types/` |
| `scheduling/schedules/` | `Projects/Scheduling/schedules/` |
| `scheduling/templates/` | `Projects/Scheduling/templates/` |
| `scheduling/bookings/` | `Records/bookings/` |
| `locations/` | `Operations/Locations/` |
| `body/` | `Projects/Fitness/body/` |
| `Wiki/`, `Wisdom/`, `Inbox/`, `Projects/` | stay (with additions) |
| `Welcome.md` | stays |

New folder notes (folder + matching `<Name>.md`):

- `Journal/Journal.md`
- `Projects/Mealplan/Mealplan.md`
- `Projects/Fitness/Fitness.md`
- `Projects/Scheduling/Scheduling.md`
- `Operations/Operations.md`
- `Operations/Locations/Locations.md`
- `Operations/Inventory/Inventory.md`
- `Records/Records.md`

## Path constants in code (option C, in-place)

Only string paths flip — crate names and module structure
unchanged.

| File | Const / literal | New value |
|---|---|---|
| `features/scheduling/scheduling/src/vault_scheduler.rs` | `VAULT_SUBDIR = "scheduling"` | drop (unused with split) or `"Projects/Scheduling"` for backward audit log key |
| same | `TEMPLATES_DIR` | `Projects/Scheduling/templates` |
| same | `EVENT_TYPES_DIR` | `Projects/Scheduling/event-types` |
| same | `SCHEDULES_DIR` | `Projects/Scheduling/schedules` |
| same | `BOOKINGS_DIR` | `Records/bookings` |
| `scheduling/src/memory.rs` | `format!("scheduling/bookings/{id}.md")` | `Records/bookings/{id}.md` |
| `features/task/task/src/write.rs` | `tasks/{slug}.md` | `Task/{slug}.md` |
| `features/inventory/inventory/src/write.rs` | `inventory/{slug}.md` | `Operations/Inventory/{slug}.md` |
| `features/locations/locations/src/write.rs` | `locations/{slug}.md` | `Operations/Locations/{slug}.md` |
| `features/mealplan/pantry/src/write.rs` | `pantry/{slug}.md` | `Operations/Inventory/Pantry/{slug}.md` |
| `features/mealplan/mealplan/src/write.rs` | `mealplan/{date}-{slug}.md` | `Projects/Mealplan/meals/{date}-{slug}.md` |
| `features/fitness/body/src/write.rs` | `body/{slug}.md` | `Projects/Fitness/body/{slug}.md` |

In-crate test assertions follow each constant.

`mealplan/mealplan/tests/end_to_end.rs:38` creates
`mealplan/` directly — flip to `Projects/Mealplan/meals/`.

Doc comments referencing old paths in `lib.rs` / `model.rs`
get updated alongside.

Out of scope for this PR:

- `features/email/email-link/` test fixtures use `tasks/` /
  `projects/` / `people/` as organizational directories
  inside tempdirs — discrimination is by frontmatter `type:`,
  not path. Left as-is.
- `features/wiki/*` — `Wiki/` stays put.
- `features/fitness/exercises` — `Wiki/Exercises/` stays put.
- `features/mealplan/cookbook` — `Wiki/Cookbook/` stays put.

## Doc updates

- `examples/vault/README.md` — full rewrite, split table
  into Notes / Workflow / Operations / Records.
- `plans/vault-feature.md` — update page count + the "Example
  vault" section.

## Verification

- `cargo check -p scheduling -p task -p inventory -p locations -p pantry -p mealplan -p body`
- `cargo test -p scheduling -p task -p mealplan` (the tests
  with hardcoded path asserts).
- `cargo run -p scheduling --example demo_scan` should still
  print the seeded event-types and bookings from the new
  locations.

## Migration note for downstream vaults

Existing on-disk vaults (the user's personal vault, any test
fixtures outside this repo) keep working only if they adopt
the new layout. There is no compat shim. A one-shot move
script for an existing vault:

    mkdir -p Journal Operations/Inventory Operations/Locations \
             Projects/Mealplan/meals Projects/Fitness \
             Projects/Scheduling Records/bookings Task
    git mv Daily Journal/Daily
    git mv Meetings Journal/Meetings
    git mv tasks Task
    git mv mealplan Projects/Mealplan/meals
    git mv pantry Operations/Inventory/Pantry
    git mv scheduling/event-types Projects/Scheduling/
    git mv scheduling/schedules Projects/Scheduling/
    git mv scheduling/templates Projects/Scheduling/
    git mv scheduling/bookings Records/bookings
    git mv locations Operations/Locations
    git mv body Projects/Fitness/body
    rm -rf Stubs Bases Fleeting

Not run automatically — each vault owner runs it once.
