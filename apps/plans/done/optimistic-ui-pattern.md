# Optimistic UI pattern — canonical write-through for list pages

> **SUPERSEDED** by the architect atom store migration — see
> [`atom-store-migration.md`](atom-store-migration.md). The in-house
> write-through list helper this plan introduced (`src/optimistic.rs`)
> and the task wiring shim were the precursors; both are deleted.
> Pages now share one pattern: `architect::Store` + `AtomResult` +
> `use_mutation` via per-feature hooks in `crates/ui/src/stores.rs`.
> Kept for history.

**Status:** superseded (was: rollout complete for the clean-fit
pages; 7 pages converted on the in-house helper).
**Scope:** every `/…` route in `crates/ui/src/pages/` that lists a vault
entity and lets the user create / edit / delete it. The lifecycle-y
pages (timer start/stop, invoice mark-paid/void) are explicitly *out*
of v1 — see Non-goals.

---

## Background — two patterns coexist today

Mutating pages currently use one of two shapes:

1. **`/tasks` — true optimistic, write-through** (`crates/ui/src/task_wiring.rs`).
   The route holds the authoritative `Signal<Vec<DbTask>>`. A mutation
   is applied to that signal *first* (instant UI), then a best-effort
   `client.{create,update,delete}` is fired on a background task. On
   failure we just `tracing::warn!` — the next load reconciles. No
   refetch round-trip blocks the UI.

2. **refresh-counter — fire-and-refetch** (`crates/ui/src/pages/locations.rs`
   and ~9 others). The route keeps a `refresh: Signal<u32>`, a
   `use_resource` subscribes to it, and every mutation does
   `spawn(async { feeds::create_x(..).await; refresh += 1; })`. The user
   stares at the old list until the server round-trip *and* the refetch
   complete, then the row appears. Two round-trips per create.

Pattern 1 is the one we want everywhere. Pattern 2 was the quick way to
ship the read+create slices; it's the debt this plan pays down.

### Why our reconcile is simpler than the textbook one

The canonical optimistic-insert example (dioxus-mcp
`optimistic-with-reconcile`) keys reconciliation by *content*
`(author, body, ts)` because the canonical row arrives later over an
**SSE stream** and you can't know which optimistic row it maps to.

Our CRUD services are **request/response**: `feeds::create_location(..)`
*returns the persisted entity* (backend-assigned `id`, vault `path`,
timestamps). The `spawn`ed write-through holds the provisional row's id
in its own closure, so reconciliation is keyed by that id — no
content-matching, no SSE. When the future resolves we replace the
provisional row in place with the returned canonical row.

(If/when vox starts *pushing* updates for these entities, we revisit and
add content-keyed dedupe like the streaming example. Not now.)

---

## The helper — `crates/ui/src/optimistic.rs`

A small, target-agnostic collection primitive: `OptimisticList<T, K>`.
It owns three signals (`rows`, `pending`, `failed`) and an `id_of` fn
pointer. It is `Copy` (so it threads into event-handler closures and
**drills into child components as a prop** — see inbox) and its methods
take `&self` (the `Signal` fields are `Copy` handles with interior
mutability), so call sites need no `mut` binding.

`K` is the id key type, **generic** — inferred from `id_of`. Most
entities key on `uuid::Uuid` (`|l| l.id`); some key on a `String`
(inbox `|i| i.id.clone()`, recipes on `path`). Two turbofish slots, so
name the first and leave the key to inference: `::<Location, _>(|l| l.id)`.

```rust
let mut list = use_optimistic_list::<Location>(|l| l.id);

// initial load — server snapshot is ground truth, clears pending/failed
use_effect(move || {
    if let Some(Ok(rows)) = &*loader.read_unchecked() { list.set(rows.clone()); }
});

// create: provisional row shows instantly; write-through reconciles to canonical
let prov = Location { id: Uuid::new_v4(), /* … */ };
let slug = slug.clone();
list.create(prov, async move { feeds::create_location(&slug, &n, &k, addr).await });

// render
for loc in list.items().read().iter() {
    let state = list.state(loc.id);           // Settled | Pending | Failed
    LocationRow { key: "{loc.id}", loc: loc.clone(), state }
}
```

### Semantics

| method | optimistic effect | on `Ok(real)` | on `Err` |
|---|---|---|---|
| `create(prov, fut)` | push `prov`, mark `pending(prov.id)` | replace `prov` in place with `real`, clear pending | clear pending, mark `failed(prov.id)`, `warn!` |
| `update(next, fut)` | replace row, mark `pending(id)` | overlay `real`, clear pending | restore previous row, mark `failed`, `warn!` |
| `delete(id, fut)` | remove row | — | re-insert at original index, mark `failed`, `warn!` |
| `set(rows)` | replace all, clear pending + failed | — | — |
| `state(id)` | — read accessor for per-row styling | | |
| `items()` | — returns the `Signal<Vec<T>>` for rendering | | |

- **Provisional key must be client-stable + unique.** `create` reconciles
  by finding the provisional row's key. So the caller mints a key the
  backend can't have collided with:
  - **Uuid-id entities** (locations/inventory/milestones/fitness): mint
    `Uuid::new_v4()` as the provisional id. Backend may keep it or assign
    its own — `create` replaces the *whole* row, so the swap is free.
  - **String/path-keyed entities** where the key is *server-derived*
    (recipes key on `path`, assigned at write): the real key isn't known
    pre-persist, so mint a unique sentinel — `format!("__pending__/{}",
    Uuid::new_v4())`. It only has to be unique for the in-flight moment;
    the canonical row (real path) replaces it on `Ok`, so it never lands.
- **Native = local-only.** On `not(target_arch = "wasm32")` there is no
  vox client; the optimistic mutation still applies to the in-memory list
  but the write future is dropped and `pending`/`failed` stay empty (no
  false "pending" rows on desktop). Mirrors `task_wiring`'s native no-op.
- **Reconcile-on-fail.** A failed write leaves the row visible and
  `failed`, never silently drops it. Re-navigating / org-switch reruns the
  loader → `set()` → clean server truth.

### Surfacing `RowState`

Pages render the entity directly (dumb-component rule: data in via props).
`state(id)` lets the page add a class (`opacity-60` while `Pending`, a
`border-destructive/40` ring while `Failed`). Surfacing is **optional** —
a page may ignore `state` and still get instant inserts. Matching the
existing `/tasks` bar requires nothing more than instant + write-through;
the pending/failed affordance is the upgrade.

---

## Rollout matrix

Exemplar first (this plan), then fan out. Each conversion: drop the
`refresh` signal, swap the `use_resource`→`use_signal` mirror for
`use_optimistic_list`, route create/edit/delete through the helper.

| page | entity (key) / feeds fns | what converted | status |
|---|---|---|---|
| `locations` | `Location` (Uuid) · create_location | create | ✅ exemplar |
| `inventory` | `Item` (Uuid) · create_item | create | ✅ |
| `milestones` | `Milestone` (Uuid) · create_milestone | create | ✅ |
| `mealplan` | `PantryItem` (Uuid), `Recipe` (path) · create_pantry_item, create_recipe | both creates (recipe uses `__pending__` sentinel key); meals read-only | ✅ |
| `fitness` | `BodyMetric`/`Exercise` (Uuid) · create_body_metric, create_exercise | both creates; workouts/intake read-only | ✅ |
| `inbox` | `InboxItem` (String) · upsert_inbox_item, delete_inbox_item | create + update + delete; helper drilled into `InboxRow`/`SuggestedRow` in place of the old `refresh` prop | ✅ |
| `bookings` | `Booking` (String) · cancel_booking | cancel = optimistic delete (added a Cancel button — see note); `EventType` create stays on refresh (blocked) | ✅ partial |
| `project_detail` | tasks tab already on `task_wiring`; other tabs refresh-counter | — | partial, deferred |

> **Note (bookings):** the page had no Cancel control before; the delete
> conversion *added* one. If unconditional per-row Cancel isn't desired,
> gate it by status or remove — the optimistic-delete wiring is the
> reusable part.

### Blocked — need a small feeds/proto change first

- **`bookings` event-type create** — `feeds::create_event_type` returns
  `Result<(), String>`, not the persisted `EventType`. `create` needs the
  entity back to reconcile. Fix: make `create_event_type` return the
  created `EventType` (like `create_location` does), then it converts like
  any String-keyed create with a `__pending__` sentinel id. Left on
  refresh-counter until then.

### Non-goals (v1) — bespoke, not list-CRUD

- **`timer`** — start/stop/active-session lifecycle, single mutable
  record not a list. Keep as-is; revisit with a dedicated `OptimisticOne<T>`.
- **`invoices`** — mark-paid / void / auto-number are server-authoritative
  lifecycle transitions; optimistic state here risks showing an invoice as
  paid before the ledger posting lands. Leave server-confirmed.
- **`inbox` ProcessReview** — the bulk-process sub-flow still refetches
  (`reload` signal) after walking its frozen queue; not drilled through
  the list. Follow-up: route its mutations through the same `OptimisticList`.

---

## Sequencing

1. **Helper + exemplar** (this change): `optimistic.rs` + `locations.rs`,
   `cargo check -p task-app-web --target wasm32-unknown-unknown` clean.
2. **Simple list+create** (parallel agents, helper is the hard contract):
   `inventory`, `milestones`.
3. **Multi-list / multi-tab** (parallel): `mealplan`, `fitness`, `bookings` —
   one `OptimisticList` per list on the page.
4. **`inbox`** — retire its bespoke `Mutation` enum onto the helper.
5. Each step: native + wasm check clean before commit.

## Acceptance criteria

- No `refresh += 1` refetch-after-mutate left on a converted page.
- Create shows the row before the network resolves; failure leaves it
  flagged, not dropped.
- `cargo check -p task-ui` and `… -p task-app-web --target wasm32-unknown-unknown`
  both clean.

## Risk register

- **Native false-pending** — mitigated: pending/failed only tracked on wasm.
- **Backend rejects client id** — mitigated: `create` replaces the whole
  row with the server's returned entity; provisional id is throwaway.
- **Multi-list pages** — each list gets its own `OptimisticList`; don't
  share one across entity types (the `id_of` and `T` differ).
- **Lost delete position on restore** — `delete` snapshots the index and
  re-inserts there; clamped to current len if the list shrank meanwhile.
