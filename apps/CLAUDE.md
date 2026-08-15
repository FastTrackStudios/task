# Claude

This project's agent instructions live in [AGENTS.md](AGENTS.md). Read
it first. Also read the repo-root `CLAUDE.md` — Task is a subtree of
the FastTrackStudio monorepo, not a standalone repo.

Key callouts (all detailed in AGENTS.md):

- **Layout**: `apps/task/{cli,server,web,desktop,mobile}`,
  `crates/task/{ui,telemetry,xtask}`,
  `features/task/<slice>/*`. The app shell is `crates/task/ui`
  (package `ui`); `task-ui` is a *different* crate
  (`features/task/task/task-ui`, the task-list components).
- **Slices are not uniform**: `<slice>-proto` + facade `<slice>` is
  the common shape, with optional `-db` / `-ui` / `-live` crates —
  but several slices are proto-only, and several (`task`, `project`,
  `goal`, `cycle`) carry their `#[architect::rpc]` trait inside the
  facade crate. Read the neighbour before copying it.
- **Storage**: markdown + YAML frontmatter in a `vault::Vault` is the
  default and the source of truth. sea-orm backs a minority
  (agent-task queue, timer, threads, prefs, finance) plus architect's
  auth / permissions / share tables. **Loro is used only for
  collaborative editing of vault markdown files** — there is no entity
  CRDT layer; `EntityCrdt` and `*RepoLoro` do not exist.
- **RPC**: `#[architect::rpc]` traits, dispatched by
  `architect::LayerRouter` per org at `/org/{slug}/vox`.
  `#[subscribe]` streams are the target idiom, but only `task` and
  `workstream` have migrated — the rest still use the older
  `tx: Tx<T>` parameter form. Migration in progress.
- **UI rules**: architect-ui primitives only (in-tree at
  `libs/architect-ui/architect-ui`), theme tokens never hex, dark mode default,
  dumb components.
- **Gotchas**: touching a proto changes vox method ids — rebuild
  task-server before trusting live behavior. `dx serve --hot-patch
  false` for wasm. Lucide names (`CircleCheck`, not `CheckCircle2`).
  `StatusBadgeVariant` is only `Success`/`Warning`/`Danger`/`Neutral`.
  `.peek()` vs `.read()` in `use_effect` to avoid update loops.
- **Tracking**: no external tracker, no bd/beads. `plans/<topic>.md`
  with a status header for big follow-ups (see `plans/README.md`),
  `// FUTURE:` comments for narrow ones, commit messages as the
  activity log.
- **Verify before done**: `cargo check -p ui` (the shell — *not*
  `-p task-ui`) and `cargo check -p task-app-web --target
  wasm32-unknown-unknown` clean. One cargo command at a time per
  worktree.
- **Logging = wide events, always**: load the `logging-best-practices`
  skill before writing any log/debug output. The request span is the
  wide event — enrich via `architect_telemetry::wide::set`, never scatter
  log lines, never `println!`/`eprintln!`/`dbg!` (not even as debug
  scaffolding — write a failing test instead). See the root CLAUDE.md
  section and `.claude/skills/logging-best-practices/rules/fts-rust.md`.

Everything else — workflow, hard rules, gotchas, the request path,
where things live, and what's known-stale — is in
[AGENTS.md](AGENTS.md) and [ARCHITECTURE.md](ARCHITECTURE.md).
