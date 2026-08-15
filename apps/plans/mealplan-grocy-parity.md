# mealplan ↔ grocy parity

**Status:** parity tracker — ongoing. Living scoreboard, never "done".

Track our `features/mealplan/` slice toward feature parity
with [grocy](https://github.com/grocy/grocy) — the canonical
self-hosted household-management app. The goal: **scan a
barcode → pantry row materializes with name + nutrition →
recipes can deduct → low-stock + expiring views just work.**

Reference checkout: `~/Development/research/grocy/`
(read-only, depth-1 clone of `grocy/grocy@master`).

## Status

All 7 phases shipped in PR
[#50](https://git.starcommand.live/FastTrackStudios/task/pulls/50)
on top of phase 0 (cookbook + pantry + mealplan trio).
Each phase is a separate commit; the plan stays as the
read-the-room reference for what each piece does + why.

- ✅ Phase 0 — cookbook + pantry + mealplan trio
- ✅ Phase 1 — barcode lookup via OpenFoodFacts
- ✅ Phase 2 — per-batch stock entries
- ✅ Phase 3 — quantity units + conversions
- ✅ Phase 4 — shelf-life math
- ✅ Phase 5 — recipe fulfillment
- ✅ Phase 6 — recipe nestings
- ✅ Phase 7 — shopping list
- ✅ Phase 8 — substitutions (3-layer: recipe-ingredient,
  pantry-item, registry rules; goal-filtered suggestions
  surfaced on every shortage — never auto-applied)
- ✅ Phase 9 — `Meal::nutrition_total` aggregation
  (sums recipe nutrition × servings) + end-to-end
  integration test covering the full scan → resolve → add
  stock → cook → fulfillment → shopping-list loop. Fixed
  a real bug along the way: `MealplanService::cook` was
  routing through legacy `pantry::consume` and ignoring
  stock entries — now uses `consume_stock` so FIFO +
  auto-open + shelf-life-after-open all apply.

The backend is feature-complete for grocy parity. Follow-up
work happens in fresh PRs: `task-cli` commands first (no UI
yet); then the `fitness` feature (depends on
`mealplan::Meal::nutrition_total` for calorie logs); a
`task-ui` route later. Recipe URL import
(schema.org/Recipe JSON-LD) and price-history aggregation
are worth their own PRs when there's demand.

## Scope

In-scope: barcode scan → lookup → pantry, batch-level stock
tracking, unit conversions, shelf-life math, recipe
fulfillment, shopping list.

Out-of-scope (defer-list, captured at the bottom): chores,
habits, batteries, tare weight, parent-product variants,
price-history caches, userfields, label printing, multi-user
permissions.

## Background — what grocy actually models

Two layers per food:

- **`products`** = master data. Name, default location,
  default purchase unit + stock unit + conversion factor,
  min stock amount, default best-before days (+ after-open
  + after-freezing + after-thawing), picture, calories,
  tare weight, parent product id, hide-on-overview flag.
- **`stock`** = per-purchase batch row. Own
  `best_before_date`, `purchased_date`, `price`, `open`
  flag, `opened_date`, `amount`, `stock_id` (group key for
  bookings).

Per-product side tables:

- **`product_barcodes`** — N barcodes per product (different
  package sizes, alternate formats).
- **`quantity_units`** + **`quantity_unit_conversions`** —
  proper unit system with conversion factors between
  arbitrary unit pairs (e.g. `1 box = 6 can`, `1 kg = 1000 g`).
- **`stock_log`** — append-only audit trail of every stock
  operation. Each row references a `transaction_id` so
  multi-row bookings undo atomically.

Adjacent domains:

- **`shopping_list`** + **`shopping_lists`** — multi-list
  shopping with auto-populate from missing / overdue /
  expired stock.
- **`recipes`** + **`recipes_pos`** (positions = ingredients)
  + **`recipes_nestings`** (recipe-references-recipe) + the
  `recipes_fulfillment` view that joins positions against
  `stock_current` to emit "can-cook + missing-by qty".
- **`meal_plan`** + **`meal_plan_sections`** — meals on a
  calendar; `consume` deducts each ingredient's stock.
- **`chores`** + **`chores_log`**, **`habits`** +
  **`habits_log`** — recurring household tasks (separate
  from food; future `fitness` may converge here).
- **`equipment`**, **`batteries`** + **`battery_charge_cycles`** —
  durable goods. Our `inventory` crate already covers the
  equipment shape.
- **`userfields`** + **`userfield_values`** — schema-extending
  custom fields per entity (skip — vault frontmatter is
  already schema-extensible).

External barcode lookup is a plugin interface
(`BaseBarcodeLookupPlugin`). The shipped
`OpenFoodFactsBarcodeLookupPlugin` hits
`world.openfoodfacts.org/api/v2/product/{barcode}?fields=…`
and returns name + image. The endpoint also exposes
`nutriments` (per-100g macros) and `serving_size` — we
extend on top.

## What changes — phased

Each phase is one PR. Stack order matters; later phases
assume earlier ones landed.

### Phase 1 — barcode scan → pantry (smallest meaningful slice)

**Goal**: scan a UPC, get a partly-populated `PantryItem`
draft back. No batches yet, no unit conversions.

- Add `barcodes: Vec<String>` to `PantryItem`. Round-trip
  through YAML; one product → N barcodes.
- New `features/mealplan/pantry/src/lookup.rs` — pure HTTP
  client against OpenFoodFacts v2. Returns
  `PantryItemDraft { name, brand, food_category,
  nutrition_per_unit, nutrition_unit, barcode, image_url }`.
  No vault touch — caller decides what to do with the draft.
- New `pantry::Store::resolve_barcode(barcode) ->
  PantryItem | PantryItemDraft` — first checks local vault
  for a match, falls back to lookup. UI workflow: "found
  draft; create / merge / cancel".
- Service-trait additions: `PantryService::find_by_barcode`,
  `PantryService::resolve_barcode`.

Acceptance: `cargo check -p pantry` clean; a CLI smoke test
or unit test resolves a known barcode against OFF
(skip-on-no-network) and returns the right shape.

### Phase 2 — stock entries (batches)

**Goal**: model "I bought another bag of pasta on Friday"
without overwriting the first. Inline list per pantry item.

- New `StockEntry` type: `{ id: Uuid, qty: f64,
  purchased_date: NaiveDate, best_before: Option<NaiveDate>,
  opened: bool, opened_date: Option<NaiveDate>,
  price: Option<f64>, location_id: Option<Uuid>, note:
  Option<String> }`.
- `PantryItem::stock_entries: Vec<StockEntry>`. The
  page-level `qty` becomes derived (sum of entry qtys) but
  stays writable as a fallback for legacy / un-batched
  pages.
- Service-trait additions: `add_stock`, `consume_stock`,
  `transfer_stock` (between location_ids), `inventory`
  (correct the count to an absolute number).
- `consume` semantics: FIFO by `best_before`, then by
  `purchased_date`. Opened entries consumed first when ties.
- Mealplan's `cook` switches to entry-aware consume so each
  `PantryDeduction` debits real batches, not the sum.

Acceptance: round-trip a multi-entry page; consume reduces
the oldest entry first; mealplan cook test confirms
batch-level audit trail on the meal page.

### Phase 3 — quantity units + conversions

**Goal**: recipes deducting `100 g` of pasta debits a `1 kg`
pantry entry. No more "unit string mismatch" bugs.

- New module `pantry::units` (or its own
  `features/mealplan/units/` crate if it grows). Canonical
  unit set: SI mass (g/kg), SI volume (ml/l), US volume
  (tsp/tbsp/cup/floz/qt/gal), count (each/clove/bunch),
  package (box/bag/can).
- `Unit` enum + `Unit::convert(qty, from, to) ->
  Option<f64>` for known pairs.
- `PantryItem` gains `purchase_unit: String` (display
  surface) and `stock_unit: String` (canonical for consume
  math) + `purchase_to_stock_factor: f64`.
- Recipe ingredient consume picks the right entry by
  matching `stock_unit`, falling back to runtime conversion
  via `Unit::convert`.

Acceptance: unit-mismatch consume succeeds when a
conversion is known; surfaces a clear error when not.

### Phase 4 — shelf-life math

**Goal**: stop hand-entering expiry dates. Defaults compute
from purchase + product knowledge.

- `PantryItem` gains:
  `default_best_before_days: Option<u32>`,
  `default_best_before_days_after_open: Option<u32>`,
  `default_best_before_days_after_freezing: Option<u32>`,
  `default_best_before_days_after_thawing: Option<u32>`,
  `due_type: BestBefore | Expires` (soft vs hard).
- On `add_stock`, if entry has no `best_before` but the
  item has `default_best_before_days`, compute
  `purchased_date + days`.
- On `consume` opening an entry, if any opened entry has
  no explicit best_before, recompute from
  `opened_date + after_open_days`.
- `scan::expiring_within(days)` view.

Acceptance: opening an entry that previously had a 1y
shelf-life shifts its effective expiry to 7d when
`after_open_days=7`.

### Phase 5 — recipe fulfillment

**Goal**: "can I make Garlic Pasta from what's on hand?"

- Pure function `mealplan::fulfillment::check(recipe,
  pantry_snapshot, servings) -> Fulfillment`. No I/O — both
  inputs are typed.
- `Fulfillment { can_cook: bool, missing: Vec<{ name, need,
  have, unit }> }`. Matches recipe ingredients to pantry
  items via barcode or name fuzzy-match; honors stock_unit
  conversions from phase 3.
- New `MealplanService::can_cook(recipe_id, servings)` —
  read-only convenience.
- New `mealplan::recipe_pos` resolver — handles
  ingredient-name → pantry-item-id once and caches per
  recipe so subsequent checks are O(ingredients).

Acceptance: garlic-pasta + a stocked pantry returns
`can_cook = true`; remove the spaghetti entry and it returns
the right shortage.

### Phase 6 — recipe nestings

**Goal**: "Pizza" references the "Pizza Dough" recipe so
ingredient lists compose.

- `Recipe` gains `nested_recipe_ids: Vec<{ recipe_id,
  servings }>` (each row is "use N servings of this
  recipe").
- Fulfillment recurses through nestings (cap depth to
  guard against cycles).

Acceptance: a 2-level nested recipe's fulfillment matches
hand-computed totals.

### Phase 8 — substitutions

**Goal**: when an ingredient is short, surface viable swaps
ranked by user goals (out-of-stock, lower-calorie, vegan,
gluten-free, cheaper, …). Three layers, evaluated in
precedence order; suggest only, never auto-apply.

- **Layer 1** (most specific): `cookbook::Substitution`
  on `Ingredient::substitutes`. Recipe-author intent for
  *this* dish. Always visible regardless of goal filter.
- **Layer 2**: `pantry::Substitution` on
  `PantryItem::substitutes`. Global pantry knowledge with
  `Vec<SubReason>` goals. Bidirectional in spirit (you
  edit it on whichever item is the natural anchor).
- **Layer 3**: `mealplan::substitutions::SubstitutionRule`
  pages (`type: substitution`) — composable knowledge
  graph. The home for cross-cutting / fitness-aware
  rules. `for_item(from_item_id)` lookup.

Wire-up: `fulfillment::check_with_subs(recipe, pantry,
rules, goals)` populates `Shortage::suggestions` from all
three layers; `goals` filters + sorts by match count.
`SubstitutionSource` enum on each suggestion tells the UI
which layer it came from.

Acceptance: registry rule "butter → coconut oil
(Vegan, LowerCalorie)" surfaces when butter is missing;
goal filter `[HigherProtein]` drops it; recipe-level subs
stay visible under any goal filter.

### Phase 7 — shopping list

**Goal**: the missing list of a planned meal hits a
shopping list. Auto-populate from low-stock + expired.

- New `features/mealplan/shopping/` crate (or as a module
  inside `mealplan/`):
  - `ShoppingList` (id, name, store_location_id) +
    `ShoppingEntry` (item_id or free-text name, qty, unit,
    note, purchased)
  - `ShoppingService` with `add_missing_for_meal(meal_id)`,
    `add_low_stock()`, `add_expired_or_overdue()`,
    `clear`, `mark_purchased(entry_id)` (→ pantry add_stock).

Acceptance: mealplan "shop for this week" produces an
accurate list against current stock + planned meals.

## Defer-list (intentional, document the why)

- **Chores / habits.** Recurring task surface. Belongs in
  a separate `features/chores/` slice once `task` grows a
  cron/recurrence shape. Don't muddle pantry with this.
- **Batteries / battery charge cycles.** Niche grocy
  feature; not in our use case.
- **Equipment.** `features/inventory/` already covers the
  durable-goods shape.
- **Tare weight.** Useful for refillable jars but rare
  enough to defer past phase 7.
- **Parent product / variants.** "Milk" parent with 1L /
  2L children. Skip until we hit a real case — bare
  `PantryItem` can model variants as siblings for now.
- **Price-history caches.** Pre-aggregations for reporting;
  on-demand `scan::price_history(item_id)` is enough until
  the dataset gets big.
- **Userfields.** Vault frontmatter is already
  schema-extensible — users add custom keys; we just don't
  parse them. No need for a schema-of-schemas layer.
- **Label printing.** Hardware integration; revisit after
  the desktop app has a print abstraction.
- **Multi-user permissions.** Auth lives in
  `architect-auth`; mealplan inherits whatever the apps
  shell does.
- **`stock_id` group key + bookings/transactions undo.**
  Phase 2's stock entries already give us per-entry undo
  via vault page rollback; multi-entry transactional undo
  is a phase-2.5 nice-to-have, not a blocker.

## Risk register

- **OpenFoodFacts coverage.** Plenty of products aren't
  in OFF. `PantryItemDraft` should be fillable manually
  with sane defaults so a missed lookup is a smooth
  fallback, not a dead end.
- **Unit conversion correctness.** Volume↔mass conversions
  depend on density (1 cup of flour ≠ 1 cup of water).
  Phase 3 keeps conversions within a single base
  (mass / volume / count); cross-base requires per-item
  density, which we defer.
- **FIFO consume ambiguity.** Two opened entries with the
  same best_before is unusual but possible. Tie-breaker:
  `opened_date` ascending, then `purchased_date` ascending,
  then `id` ascending. Document and don't surprise.
- **Recipe fuzzy match.** "olive oil" in a recipe vs
  "California Olive Ranch Extra Virgin Olive Oil" pantry
  page. Phase 5 starts with exact-id + lowercase substring
  match; tighten later.

## When in doubt — grocy is the reference

Schema: `~/Development/research/grocy/migrations/`.
API: `grocy.openapi.json` (also visible at
`https://demo.grocy.info/api`).
OpenFoodFacts plugin:
`plugins/OpenFoodFactsBarcodeLookupPlugin.php`.

Look there before inventing.
