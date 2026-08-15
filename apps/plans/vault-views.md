# Vault views — every page is a view over the vault

**Status:** direction approved (Cody, 2026-07-03), slices pending.
The Obsidian thesis applied to the whole app: the vault is the one
connected substrate; feature pages are **views**, not places.

## The model (Cody's framing)

- **Custom UIs stay custom.** The task board (Now bar, relevance),
  dashboard grid, timer, kanban — none of it gets dumbed down to a
  generic table. They become registered **view types**.
- **A vault entry declares the view.** A `.base` file binds
  `{ view type + base query/config }`. Opening it — from the vault
  tree, a `[[wikilink]]`, a backlink, the graph — renders the custom
  view in place, like any note.
- **Views are referenceable.** `[[Tasks]]` in any note links to the
  live board. The knowledge graph sees it. Everything participates.
- **Sidebar items are shortcuts** to vault entries, not routes.
  Navigation collapses toward `/vault/:path`; what's pinned becomes
  vault/user config (the prefs entity), not compiled-in tabs.
- **Databases embed when needed.** sqlite-backed systems (timer,
  finance, threads, agent, prefs) stay databases; their data appears
  in views by reference/embed, not by migration.

## Ground truth (surveyed 2026-07-03)

- **Vault-backed already**: tasks, projects, goals, milestones,
  locations, inventory, fitness, mealplan, workstreams. The
  unification is presentation + navigation, not data migration.
- **Base engine works**: `.base` files with `filters` (full
  and/or/not + comparisons + file./note./formula. operands),
  `views[]` (type/name/filter/order/sort/limit/groupBy/extras),
  properties metadata. Server executes (`vault_live::bases`,
  `VaultSync::base_views`), UI renders table/cards/list.
- **Idle inventory**: view-kanban / view-calendar / view-table /
  view-heatmap are finished dumb widgets with mutation events,
  wired nowhere. `board`/`calendar` view kinds parse but fall back
  to table.
- **Known inert spots**: formulas parse but evaluate to Null;
  `this` (templated bases) inert; views are read-only; view-state
  changes don't persist.

## Architecture

Two view classes, one registry:

1. **App views** — the existing custom pages, registered by kind
   (`tasks-board`, `dashboard`, `timer`, `inbox`, …). They keep
   their optimistic stores + mutations wholesale. The `.base` entry
   contributes identity (a vault path — linkable), optional base
   filter context (e.g. a tasks-board scoped to `#music` pages),
   and `extras` config.
2. **Generic base views** — table/cards/list (today) + kanban /
   calendar / heatmap (wire the idle widgets). Rows come from the
   base engine; mutations write **frontmatter through the vault
   write path** — one write path, so a kanban drag is a `status:`
   edit any other surface sees.

Registry lives in crates/ui: `view kind → renderer` dispatch inside
the vault page's `.base` handling. Unknown kinds keep the table
fallback (forward compatibility with hand-authored files).

## Slices

1. **Registry + first app view.** Dispatch `.base` view kinds to
   registered renderers; register `tasks-board`; seed
   `Views/Tasks.base`; point the sidebar Tasks item at the vault
   entry. Proves the whole loop (open-as-note, wikilink to a live
   board, sidebar-as-shortcut) end to end on one page.
2. **Wire the idle widgets** as generic kinds: `board` → view-kanban,
   `calendar` → view-calendar, `table` upgrade → view-table
   (read-only first). Kills the fallback-to-table gap.
3. **Write-back from generic views**: frontmatter edits via vault
   mutate — kanban drag sets `status:`, checkbox column runs the
   task click-cycle (`task::click_transition`), calendar drag sets
   `scheduled:`. Per-row actions come from the entity parse
   (a row that parses as a task gets task affordances).
4. **Sidebar as shortcuts**: pinned entries from a vault workspace
   config + per-user pins on the prefs entity; legacy routes become
   redirects to their vault entries. Register the remaining app
   views (dashboard, inbox, timer, schedule, fitness, mealplan, …)
   as they're pointed at.
5. **Formulas + `this`**: evaluate formula expressions; `this`
   context enables templated bases — "tasks for *this* project"
   makes the project-detail internals expressible as embedded views.
6. **Embeds for sqlite systems**: timer/finance/threads summaries
   embeddable in notes and views by reference (`![[...]]`-style),
   completing "plain text with databases embedded when needed".

## Non-goals

- No data migration (vault-backed features already are).
- No dumbing custom pages down to generic tables.
- No breaking existing URLs mid-migration (redirects until slice 4
  completes).
