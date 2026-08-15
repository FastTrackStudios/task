# Plan: domain & business logic behind architect services

**Status:** proposed (audit done, awaiting scope confirmation)
**Owner:** —
**Date:** 2026-06-14

## Goal

Every piece of domain/business logic should live behind an `#[architect::rpc]`
service (reachable by any frontend) or in the pure domain crate it belongs to —
never re-derived inside a single frontend. Client state should ride
`architect::atom` (`Store` + `use_mutation` + `AtomResult`/`use_async`) rather
than hand-rolled `use_signal` + `spawn` fetch/refresh bookkeeping.

This was prompted by the #70 cook-receipt/can-cook work, where a have/missing
partition had been derived in the CLI and was about to be re-derived in the UI;
it was moved into `fulfillment::check` so both frontends render one
server-computed result. This plan generalises that sweep.

## Audit result (verified)

Service coverage is already high: ~20 features expose `#[architect::rpc]`
traits. (An automated pass wrongly claimed `task` has no service — it does:
`features/task/task/src/service.rs:116`. Treat the raw audit with skepticism;
the items below are hand-verified.)

### Tier 1 — real logic a peer frontend needs (worth doing)

1. **CLI due/overdue + inbox review-queue filtering** — `apps/cli/src/brief.rs`
   (~108-129, ~174-181) filters open tasks due ≤ today and the inbox
   resurface queue locally. The web "today/home" surface wants the same
   predicate. → add query methods (`TaskService::list_due(date)` /
   `InboxService::review_queue(date)`), render from them in CLI + UI. Size: M.
2. **Timer session → day-plan block attribution** — `apps/cli/src/plan.rs`
   `attribute_sessions` (~489-510) splits sessions across blocks for
   plan-vs-actual. CLI-only today; the calendar/plan UI needs it. → pure fn in
   `scheduling-proto`/`timer-proto` or a service method. Size: M.
3. **Shopping list entry ops** — `apps/cli/src/mealprep.rs` `find_entry` +
   read-modify-write of the whole list for mark-purchased/remove (~516-541).
   → `ShoppingService` methods so the UI doesn't reimplement. Size: M.
4. **Finance time/revenue rollup** — `crates/ui/src/pages/finances.rs` (~72-102)
   aggregates timer sessions into billable revenue client-side. No finance
   service method covers it (ledger + invoicing exist, no time-summary). → add
   a summary method to the finance or timer service. Size: M. Needs care.
5. **Day-plan reconcile orchestration** — `apps/cli/src/plan.rs` choreographs
   fetch-templates → `merge_template` → `reconcile`. Helpers are already in
   `scheduling_proto::resolve`; wrap as one `reconcile_day(date)` method so the
   UI gets the same result in one call. Size: S–M.

### Tier 2 — state onto architect::atom

6. **schedule.rs DayPlan state** — `crates/ui/src/pages/schedule.rs` holds plans
   in `use_signal(HashMap)` populated via `use_effect` + `spawn`, with manual
   re-writes on save (no optimism). → `Store<DayPlan>` + `use_mutation`, matching
   mealplan/inventory/projects. Size: L. Highest UX payoff.

### Tier 3 — explicitly LEAVE (over-flagged by the audit)

- `crates/ui/src/task_sort.rs` priority/status ranking — UI-only display
  ordering over already-client-cached Store data; **not** duplicated in the CLI.
  Moving to an RPC would add a round-trip per render. Keep. (If the CLI ever
  needs identical ordering, promote the rank fns to `task-proto` as pure
  helpers — no RPC.)
- `goals.rs` hierarchy traversal, `projects.rs` grouping, `ledger.rs` account
  grouping, `goals.rs::cycle_label_for` — display-layer derivations over cached
  data (cycle generation is explicitly blessed as local in AGENTS.md). Keep.
- Ephemeral component state (cook-mode timers, form inputs, vault editing flags)
  — correct as `use_signal`. Keep.

### Open question — features without an rpc service

`label`, `mount`, `attachments`, `workflows` have proto types but no service.
Most are defensible (label = value type; mount = server-private fs/toml config;
attachments = upload orchestration). `workflows-orchestrator` is worth a look to
confirm its logic isn't stranded. No action unless a frontend needs them.

## Sequencing

Each Tier-1 item is an independent PR (proto change ⇒ server rebuild for schema
skew — see AGENTS.md). Suggested order by value/risk: 1 → 5 → 3 → 2 → 4, then
the Tier-2 schedule.rs atom migration (6) as its own arc.

## Acceptance

- The moved logic has one implementation, called by both CLI and UI.
- `cargo check` native + wasm clean; `cargo test -p <crate>` for touched crates.
- No new client-side derivation of domain data; no new `use_signal`+`spawn`
  fetch where `use_async`/`Store` fits.

## Risk

- Proto changes ⇒ schema skew; rebuild task-server before live testing.
- Over-correction: pushing display-layer sorting/grouping to RPCs regresses
  responsiveness. Tier 3 is deliberately out of scope.
