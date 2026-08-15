# UI anti-pattern audit — 2026-07-02

**Status:** historical audit (2026-07-02). Point-in-time lint sweep, not a work item. Re-run the sweep rather than trusting these counts.

Full dioxus-mcp lint sweep over every UI crate (crates/ui, task-ui,
finance-ui, scheduling-ui, threads-ui, view-*). 243 raw findings in
crates/ui; most were cross-crate false positives. What was real, what
was fixed, what's deferred.

## Fixed in this pass

- **27 missing `key:` on rsx for-loops** (crates/ui, 19 files) — the
  one true correctness class: unkeyed rows swap DOM state on
  insert/remove/reorder (inputs keep the wrong values, expanded rows
  jump). Keys added from stable identities (osis/path/id/slug; index
  only for skeleton + positional-cell loops). Bonus real bug: vault's
  `render_node` root carried no key at all — keyed at the source,
  fixing both flagged call sites. 4 of the original 31 were
  expression-child false positives (helpers key their roots).

## False positives (verified, no action)

- `props_lint missing_partial_eq` (57 + 4 + 3 across crates): the
  lint only sees in-crate impls; Uuid/ProjectInfo/Recipe/Thread/
  Message/AgentRef/Transition/Activity all derive PartialEq in their
  home crates. Memoization works.
- `dead_components` on the 23 `*Route` components + `AppShell`:
  reached via the `#[derive(Routable)]` enum / `#[layout]`, which the
  lint doesn't trace.
- `polling_future_no_backoff` (presence.rs ×2, view-calendar
  now-line): local heartbeat/clock tickers, not server retry loops —
  constant cadence is the point.
- task-ui markdown.rs missing keys: `render_block`/`render_inline`
  set `key:` on their root nodes internally (14 sites).
- `signal_lint hydration_unsafe_effect` (vault.rs ×2): flags an
  SSR hydration flash — the web app is client-only (no SSR), so no
  flash exists. Revisit if/when SSR lands
  (plans/web-ssr-investigation.md).

## Deferred (real, but refactors — not lint-fix material)

1. **schedule.rs `plans`/`soft_ids` written from 5 sources**
   (use_effect + assign_block/move_block/reset_day/save_block) — the
   canonical "lift to Store" smell and the audit's highest-risk state
   surface. Fold into the atom-store pattern with named mutators like
   every other page. Medium refactor; the page works today.
2. **stores.rs 16 copy-paste store modules** — the drift generator.
   Consolidation candidate when architect grows a multi-org store
   derive (see atom-store-migration.md's "hand-written because
   slug-routed" caveat).
3. **`TagIconView` (tag_icon.rs) unwired** — built for the tag-icon
   system (plans/tagging-and-icons.md) but nothing renders it yet.
   Wire when tags grow icon pickers; don't delete.
4. **`ProjectThemeScope` (theming.rs) unwired** — per-project theming
   built but never mounted; a silent feature no-op. Product decision:
   mount it on /projects/:id or remove.
5. **view-gantt bars.rs:380 `autofocus`** — E0034-ambiguous attribute
   form; compiles today, tidy to explicit syntax on next gantt touch.
6. **`reinvented_widget` hints (29)** — bare inputs/selects where
   architect-ui catalog widgets exist; the quick-adds deliberately use bare
   inputs (house style); revisit per-page during design passes.
