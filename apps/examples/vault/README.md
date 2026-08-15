# Example vault

Small Obsidian-shaped vault tracked in-repo so tests, the playground,
and the CLI have a consistent fixture to point at.

## Layout — 9 top-level folders

Notes (human-authored):

| Folder      | What |
|-------------|---|
| `Inbox/`    | Capture target. Unsorted, fast jotting. Fleeting notes evaporate or graduate. |
| `People/`   | Personal relationships / contacts (or org employees). **Not** public figures — those live in `Wiki/entities/`. |
| `Wiki/`     | Reference + concepts: transferable knowledge for this vault. Includes `Wiki/Cookbook/` (recipes), `Wiki/Exercises/` (exercise reference), `Wiki/entities/` (public figures, named things), `Wiki/raw/sources/` (LLM-Wiki ingest inputs), `Wiki/media/` (extracted images), `Wiki/_state/` (LLM-Wiki backend state). |
| `Wisdom/`   | Atomic zettels — one claim per note. |
| `Journal/`  | Dated, reflective notes. `Journal/Daily/` (`YYYY-MM-DD.md`), `Journal/Meetings/` (`YYYY-MM-DD <topic>.md`). |

Workflow:

| Folder      | What |
|-------------|---|
| `Projects/` | Active work containers, one folder note per project. `Projects/Mealplan/`, `Projects/Fitness/`, `Projects/Scheduling/`, plus single-file projects like `Projects/Editor.md`. |
| `Task/`     | Task notes (TaskNotes-shape frontmatter). `Task/open-tasks.base` views the folder. |

Instance / asset registry:

| Folder      | What |
|-------------|---|
| `Operations/` | Things that exist in the world. `Operations/Locations/`, `Operations/Inventory/`, `Operations/Inventory/Pantry/`. Referenced by projects via wikilinks. |

History:

| Folder      | What |
|-------------|---|
| `Records/`  | Append-only history. `Records/bookings/` (emitted by the `scheduling` crate). Future: time entries, audit, completed agent-task receipts. |

## Conventions

Each folder carries a folder note (e.g. `Wiki/Wiki.md`,
`Journal/Journal.md`) acting as the virt-folder parent. Notes carry
`folder: "[[Parent]]"` in their frontmatter pointing at that note —
single value, since multi-folder navigation is handled by tags
instead.

`.base` files (view DSL) live colocated with the data they query —
not in a separate `Bases/` directory:

- `Task/open-tasks.base`
- `Projects/Mealplan/mealplan.base`
- `Journal/Daily/daily-plan-blocks.base`
- `Records/bookings/upcoming-bookings.base`
- `Wiki/all-concepts.base`

The notes exercise wikilinks, section refs, Logseq-style block refs
(`((uuid))`), embeds, frontmatter properties, Obsidian tags, mermaid +
math blocks, and daily notes.

## Wiki vs Operations

The split that matters most:

- **`Wiki/<X>`** — `X` as a concept. Specs, theory, transferable
  knowledge. `Wiki/SM7B.md` describes the mic generally.
- **`Operations/Inventory/<X>`** — `X` as your instance. Serial,
  location, condition. `Operations/Inventory/sm7b-3.md` is *your*
  SM7B #3.

Federation: a personal vault and a business vault are different
servers; the org's gear catalog lives at the top of the business
vault's `Operations/`, not nested under a `People/<Org>/` entry. No
`Orgs/` namespace needed.

## Used by

- `features/editor/examples/playground` — `init_vault()` prefers this
  directory over `~/Documents/Task` so the playground always has
  cross-doc resolution wired even on a fresh clone.
- `apps/cli` (the `vault-*` subcommands) — point the CLI at
  `examples/vault/` for quick smoke checks:
  `cargo run -p task-cli -- vault tasks --root examples/vault`.
- `features/vault/vault-obsidian` integration tests — the
  Observatory-scale `tests/smoke_observatory.rs` is gated behind a
  present-only check; this small vault is the always-on in-tree
  fixture for write-up examples.
- Feature crates that read/write vault content all target the layout
  above by default (`scheduling`, `task`, `mealplan`, `pantry`,
  `inventory`, `locations`, `body`).

Editing these files commits to git like any other tracked content.
For day-to-day notes, point your editor at a different directory.

See [`plans/vault-reorg.md`](../../plans/vault-reorg.md) for the
mapping from the prior layout.
