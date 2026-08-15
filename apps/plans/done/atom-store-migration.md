# Atom store migration — ONE state pattern for route pages

**Status:** shipped (issue `ebba2824`). Supersedes
[`optimistic-ui-pattern.md`](optimistic-ui-pattern.md) (the in-house
`OptimisticList` precursor), the `task_wiring` shim, and every
refresh-counter (`reload += 1`) page.

## The pattern

Client state is the architect atom stack — `architect::Store`,
`AtomResult`, `use_mutation`, `use_store_list` / `use_store_entry` —
wired through per-feature hooks in `crates/ui/src/stores.rs`, hand-
written to mirror the derive-emitted idioms of the architect example
app (`../architect/examples/app`). Hand-written because Task's
reads/writes are **slug-routed** through the multi-org `feeds::*`
fan-out (per-org sockets via `vox_clients::caller_for`), which the
`#[architect(store)]` derive can't know.

Per feature:

- `provide_<entity>_store()` — one shared `Store` at the app root
  (`stores::provide_stores()` in `app.rs`, after
  `architect::use_app_supervised` so mutations find the notifications
  + reactivity registries).
- `use_<entity>_list()` — the rows as one `AtomResult<Vec<(Id, T)>>`.
  Stale-while-revalidate: an org switch re-runs the fetch (the loader
  closure reads the selection signals) while the last rows stay
  rendered (`Reloading`), never a blank "Loading…".
- `use_<entity>_mutations()` — optimistic writes via `Mutation::run`:
  patch the store synchronously (insert/update/remove), then reconcile
  against the server's returned entity or roll back. Failures surface
  in the app `Notifications` tray (`shell/app_shell.rs`), not
  `tracing::warn!`.
- In-flight inserts are typed `Id::Temp` placeholders (rendered
  dimmed via `id.is_temp()`) — no `"__pending__/<uuid>"` magic-string
  sentinels.

Multi-org views wrap rows in slug-tagged pairs (`OrgTask`,
`OrgProject`, `OrgSession`, `OrgInvoice` — `StoreEntity` impls local
to `stores.rs`) so a mutation routes back to the owning org's service.
Single-org registers implement `StoreEntity` directly on the proto
entity, behind each proto crate's new `atom` feature
(`atom = ["architect/atom"]`).

## What each page runs on now

| Page | Before | After |
|---|---|---|
| `/tasks` | resource→signal mirror + `task_wiring` write-through (warn-on-fail) | `use_task_list` + `TaskMutations::apply` |
| `/projects` | plain resource | `use_project_list` (hydrates the shared store) |
| `/projects/:id` | one big overview resource + mirrors + `page_refresh`/`threads_refresh` counters | `use_project` (store entry, cache-first), task store + live event fold, thread/message stores, `ProjectMutations` |
| `/inbox` | `OptimisticList` + `reload` counter after ProcessReview | `use_inbox_list` + `InboxMutations` (incl. promote-to-task/note) |
| `/locations` | `OptimisticList` | `use_location_list` + `LocationMutations` |
| `/inventory` | `OptimisticList` | `use_item_list` + `ItemMutations` |
| `/milestones` | `OptimisticList` | `use_milestone_list` + `MilestoneMutations` |
| `/fitness` (Body, Exercises) | `OptimisticList` ×2 | `use_body_metric_list` / `use_exercise_list` + mutations |
| `/mealplan` (Recipes, Pantry) | `OptimisticList` ×2 (recipes used `__pending__` paths) | `use_recipe_list` / `use_pantry_list` + mutations |
| `/bookings` | `OptimisticList` (bookings) + refresh counter (event types) | `use_booking_list` / `use_event_type_list` + mutations |
| `/timer` | refresh counter, separate active-timer fetch | `use_session_list`; the running session is *derived* from the one list; `TimerMutations` |
| `/invoices` | refresh counter | `use_invoice_list` + `InvoiceMutations`; the derived uninvoiced view refetches via the `finance.uninvoiced` reactivity key |
| app chrome (top-bar chips, timer widget, fleeting modal) | `Refresh` context counter bumped after capture/start/stop | chips + widget derive from the inbox/session stores; capture + start/stop go through the same mutations as the pages |

Read-only sections (fitness workouts/intake, mealplan meals, the
milestone-form project picker, project-detail budget / active-now /
repos aggregates) stay plain `use_resource` — nothing to cache or
mutate.

## Pattern-D prerequisites that were changed

- `feeds::create_event_type` now returns the persisted `EventType`
  (was unit), so the optimistic insert can reconcile.
- `feeds::create_{location,item,milestone,body_metric,exercise,recipe,
  pantry_item}` take the caller-built draft entity (the literal moved
  to `stores::draft_*`), removing the page/feeds double literal.
- `feeds::fetch_projects_tagged` — slug-tagged sibling of
  `fetch_tasks_tagged`, feeding the project store.

## Invariants

- Never create signals in keyed children that longer-lived closures
  capture (see `crates/ui/src/collab.rs`); the store hooks keep all
  signals page- or app-root-owned.
- `use_resource` polls its future in a reactive context: the loader
  closures *deliberately* read the org-selection signals so an org
  switch refetches; anything that must not re-trigger uses `peek`.
- Vault page + collab/presence code untouched (multiplayer-suite).
