# cooklang migration

Migrate `features/mealplan/` to use [cooklang](https://github.com/cooklang) as the **first-class recipe format**.

Today: recipes are `markdown + YAML frontmatter` files under `Wiki/Cookbook/`. Ingredients are a YAML sequence of `{ name, qty, unit, pantry_item_id, ... }`. The body is free-form markdown.

After: recipes are **single `.cook` files** (cooklang syntax) parsed by [`cooklang`](https://crates.io/crates/cooklang). The recipe is a **wiki reference** — it knows nothing about our pantry. Ingredient names ARE wikilink targets; resolution to pantry items happens at the mealprep layer, not in the recipe.

Reference: [cooklang spec conventions](https://github.com/cooklang/spec/blob/main/conventions.md), [cookcli](https://github.com/cooklang/cookcli), [cooklang-find](https://github.com/cooklang/cooklang-find).

---

## Why

- **Ingredient parsing is solved.** `@flour{500%g}`, scaling, unit conversion, aisle.conf grouping, pantry exclusion all live in `cooklang` already (cargo features `shopping_list`, `pantry`, `aisle`). We currently re-implement subsets of each.
- **Ecosystem leverage.** LSP, VSCode/Obsidian/Sublime/Emacs editors, tree-sitter, HomeAssistant integration, a published spec, a federated search prototype, an importer with LLM fallback — all work against `.cook` files. Our recipes become portable.
- **Better authoring.** The cooklang line `Combine @flour{500%g} and @water{300%ml}, knead for ~{10%minutes}` is denser and more natural than a YAML ingredients block + separate markdown steps.
- **Photo conventions.** `Recipe.jpg`, `Recipe.0.jpg` (per-step) is what `cooklang-find` already understands; our UI gets step-by-step photo support for free.

---

## Scope decisions (already taken)

| Question | Answer |
|---|---|
| Library vs subprocess? | **Library only.** Depend on `cooklang` crate. No `cookcli` binary shell-outs. |
| Pull in `cookcli` modules? | Maybe later for menu/.shopping-list/.shopping-checked file formats (MIT, has `[lib]`). Skip the Axum/askama web layer + i18n — we have Dioxus. |
| `cooklang-find`? | **Yes**, for the cookbook directory walker + image association — saves re-implementing the convention. |
| `cooklang-reports`? | Not now. Revisit if templated nutrition labels become a requirement. |
| `cooklang-import`? | Not now (heavy LLM deps). If added, feature-gate. |
| `cooklang-sync-client`? | **No** — we have Loro for sync. |
| Spec extensions to enable? | All of them: `MULTILINE_STEPS`, `COMPONENT_MODIFIERS`, `COMPONENT_NOTE`, `COMPONENT_ALIAS`, `SECTIONS`, `TEXT_STEPS`, `ADVANCED_UNITS`, `MODES`, `TIMER_REQUIRES_TIME`, `INTERMEDIATE_PREPARATIONS`. Use `Extensions::all()`. |

---

## On-disk format

Current vault layout:

```
<vault>/Wiki/Cookbook/<slug>.md   # type: recipe, full YAML + markdown body
```

After migration:

```
<vault>/Wiki/Cookbook/             # recipes are wiki pages in cooklang form
├── config/
│   └── aisle.conf                 # cooklang aisle groupings (shopping-list section assignment)
├── <slug>.cook                    # canonical recipe — pure cooklang. The whole recipe.
├── <slug>.jpg                     # title image (cooklang-find convention)
├── <slug>.0.jpg                   # step images
└── Plans/
    └── 2026-W21.menu              # cookcli `.menu` format for weekly plans (later)
```

The cookbook is **part of the wiki**, not a sibling. Recipes
co-locate with the entity / concept pages they reference so
ingredient wikilinks resolve in the same namespace.

**One file per recipe. No sidecar.** The `.cook` file is portable to any cooklang tool (cookcli, Obsidian plugin, VSCode, HomeAssistant, etc.) without our metadata polluting it.

### What lives in the `.cook` file

Cooklang's [metadata block](https://cooklang.org/docs/spec/#the-metadata-specification) carries everything we need on the recipe itself:

```cooklang
>> title: Truffle Pasta
>> description: Elegant carbonara variant
>> servings: 2
>> course: dinner
>> cuisine: italian
>> prep time: 5 minutes
>> cook time: 15 minutes
>> tags: weeknight, pasta
>> source: https://example.com/truffle-pasta

Bring a pot of water to a boil and add @salt{1%tsp}.
Cook @pasta{200%g} for ~{8%minutes} until al dente.
Meanwhile, melt @butter{20%g} in a pan over low heat — see [[saute]] for technique.
Toss pasta with butter and shaved @truffle{5%g}.
```

### Ingredient names ARE wikilinks

`@pasta` is both a cooklang ingredient AND a wikilink to `[[pasta]]` in the vault. Resolution is **pure render-layer convention** — no spec change, no parser change.

- Renderer: ingredient name → look up wiki page by name → if found, link to it; if not, render as plain ingredient.
- Pantry matching: mealprep layer takes each ingredient name and matches against pantry items (by name, fuzzy, or via the wiki page if the wiki page IS a pantry item). No `pantry_item_id` ever appears in the `.cook` file.
- Concept links: `[[saute]]`, `[[mise en place]]`, etc. appear as plain text inside step bodies. Cooklang preserves arbitrary text in steps, so these pass through the parser unchanged. Our renderer treats them as wikilinks.

### What does NOT live in the file

- **No UUIDs.** Recipes are identified by their relative path inside `Cookbook/` (cooklang's convention). Meals reference recipes by path: `Cookbook/Truffle Pasta.cook`. Renames update meal files (we own both sides; this is a solved problem with our existing rename pipeline).
- **No `pantry_item_id` links.** Pantry resolution happens at mealprep time by name match.
- **No substitutions.** Substitution rules live in our existing `mealplan/src/substitutions.rs` registry as separate vault entities; they apply at mealprep time against ingredient names. The `.cook` file just says `@butter{20%g}` — the registry knows butter → olive oil with ratio 0.75.
- **No precomputed nutrition.** Computed at view time from `@ingredient{qty%unit}` lines against the pantry's per-unit nutrition (already on `PantryItem::nutrition_per_unit`). If a recipe has ingredients with no matching pantry item, nutrition shows as partial — that's the honest answer.

### Meals + pantry items

Unchanged on disk. `Meal::recipe_ids: Vec<Uuid>` flips to `Meal::recipe_paths: Vec<String>` (relative paths inside `Cookbook/`). Rename a recipe → existing rename pipeline rewrites referencing meals.

---

## Crate shape

Today:

```
features/mealplan/
├── cookbook/    # parse/write recipe.md, Recipe struct, CookbookService
├── mealplan/    # Meal + fulfillment + shopping + substitutions + facade
└── pantry/      # PantryItem + units + barcode lookup
```

After:

```
features/mealplan/
├── cookbook/                       # SHRUNK
│   ├── src/cooklang.rs             # NEW: cooklang::Parser wrapper, AST → Recipe
│   ├── src/model.rs                # Recipe = thin wrapper over cooklang AST + path
│   ├── src/scan.rs                 # walk Cookbook/ for *.cook (use cooklang-find)
│   ├── src/service.rs              # CookbookService — same trait surface, path-keyed
│   └── src/store.rs                # path resolution + image association
├── mealplan/                       # mostly unchanged
│   ├── src/fulfillment.rs          # adapter: scaled cooklang ingredients → name-match pantry
│   ├── src/shopping.rs             # CONSIDER: use cooklang's shopping_list feature
│   └── src/substitutions.rs        # unchanged (registry rules match by name)
└── pantry/                         # unchanged
    └── src/units.rs                # CONSIDER: replace with cooklang::convert::Converter
```

Deleted (replaced by cooklang):

- `cookbook/src/parse.rs` — `cooklang::Parser` does this.
- `cookbook/src/write.rs` — `.cook` files are authored by humans (or our migration tool, one-shot). Service mutations re-render via cooklang's printer, or we just refuse to mutate recipe bodies through the service and leave authoring to file edits.
- Most of `cookbook/src/model.rs` — `Recipe` becomes `{ path: String, ast: cooklang::ScalableRecipe }` plus convenience accessors.

### Schema migration on the wire

Today, `Meal` carries `recipe_ids: Vec<Uuid>` and ingredients are full structs. After:

- `Meal { ..., recipe_paths: Vec<String> }` (path is relative to vault's `Cookbook/`)
- `Recipe` no longer has an `id` field on the wire — `path` is the identity.
- `proto` for `mealplan` updates accordingly; this IS a breaking wire change. No back-compat shim — we own all consumers.

---

## Migration phases

Each is one commit, gated by `cargo check -p mealplan` clean.

### Phase 1 — add cooklang dependency, parallel parse path

- Add `cooklang = "0.18"` to workspace.
- Add `cookbook/src/cooklang.rs` with `parse_cook_file(path) -> Result<CooklangRecipe>`.
- Add `Recipe::from_cook(cooklang_recipe, sidecar)` constructor. Existing `Recipe::from_markdown` path stays.
- Unit tests: parse the cooklang sample corpus (one test per ingredient/timer/cookware feature).
- **Verify**: `cargo check -p cookbook` clean. No behavior change.

### Phase 2 — path-as-id + meal schema update

- `Recipe::id` removed. `Recipe::path: String` is the identity (relative to `Cookbook/`).
- `Meal::recipe_ids: Vec<Uuid>` → `Meal::recipe_paths: Vec<String>`.
- Update `mealplan/proto`, all callers, and `mealplan/tests/end_to_end.rs` fixtures.
- **Verify**: `cargo check -p mealplan` clean.

### Phase 3 — scan + service for `.cook` layout

- `cookbook/src/scan.rs`: prefer `cooklang-find` for tree walk + image association; fall back to manual `walkdir` if it doesn't fit our path model.
- `CookbookService` trait surface: `list/get(path)/create(path, body)/update(path, body)/rename(path, new_path)/delete(path)`. Body is raw cooklang text.
- `rename` also renames sibling `<slug>.*.jpg` images and rewrites any `Meal` files that reference the old path.
- **Verify**: end-to-end test creates 3 recipes, scans, gets each, edits one, deletes one.

### Phase 4 — fulfillment adapter (name-match only)

- `mealplan/src/fulfillment.rs`: scale cooklang recipe via `cooklang::ScaledRecipe`, then run `check()` against scaled ingredient list.
- Pantry match: **name only** (case-insensitive). The wiki page name is the join key. If the user names their pantry item "Pasta" and the recipe says `@pasta`, they match. This is the entire integration point — no IDs in the recipe file.
- Unit conversion: try `cooklang::convert::Converter` first; fall back to our `pantry::convert_str`.
- Substitutions: registry rules still match by ingredient name; surface as before.
- **Verify**: existing `mealplan/tests/end_to_end.rs` passes after converting fixture recipes to `.cook` syntax.

### Phase 5 — shopping list (optional, evaluate)

- Try `cooklang`'s `shopping_list` feature: combines multi-recipe ingredients, applies aisle.conf, excludes pantry.
- Compare output to our `mealplan/src/shopping.rs`. Decide: replace, wrap, or keep ours.
- Decision deferred to phase 5 — both have merits.

### Phase 6 — vault + wiki integration (the wikilink layer)

**Status:** cookbook-side bridge shipped (`cookbook::recipe_wiki_edges`); vault/wiki indexing is wiki-feature work tracked separately.

- ✅ `cookbook::wiki::recipe_wiki_edges(recipe) -> Vec<WikiEdge>` projects each `@ingredient` / `#cookware` / `@@recipe-ref` into a `(source_path, target_basename, kind)` edge. Wiki indexers call this on every `.cook` file they discover and feed it into the same edge store handling markdown `[[...]]` links. So `Cookbook/Pasta.cook` containing `@flour{500%g}` produces edge `Pasta.cook → flour`, and the backlinks pane for the wiki/pantry page named "flour" lists every recipe that uses it.
- ⏳ `vault::Vault::scan()` discovering `Cookbook/*.cook` — needs a new `VaultEntryKind::Cook` plus walker + snapshot field changes. Tracked separately as wiki-feature work.
- ⏳ Wiki search indexing `.cook` files (title from `>> title:` metadata or filename; body text from steps). Depends on the vault-side change.
- ⏳ `[[ChickenFrench]]` in a markdown page resolving to `Cookbook/ChickenFrench.cook`. Depends on the vault-side change.

### Phase 7 — UI surfaces

- Recipe view: render cooklang AST with our Dioxus components. Ingredients table on the left, scaled steps with embedded `@ingredient` highlights on the right, step images inline.
- Authoring: textarea with cooklang syntax + live preview. Defer LSP integration; users get plain text editing initially.
- Server-side scaling slider, photo-per-step rendering.

### Phase 8 — migration tool

- `cargo run -p cookbook --bin migrate-md-to-cook -- <vault>` converts existing `Wiki/Cookbook/*.md` → `Cookbook/*.cook`. Idempotent. Leaves originals in place (delete after manual review).
- For each existing recipe:
  - YAML frontmatter scalar fields → cooklang metadata block (`>> servings:`, `>> course:`, etc.).
  - Body markdown → numbered steps → cooklang lines. For each ingredient in the YAML `ingredients:` list, find its name in the step text and rewrite to `@name{qty%unit}`. If not found in any step, append an `@name{qty%unit}` to the first step (so cooklang still picks it up).
  - Tags, source, description → metadata block.
  - Nutrition: drop. Recomputed at view time from pantry data.
  - Existing `pantry_item_id` links: drop. Name match takes over.

---

## Risks / open questions

- **Cooklang metadata is single-key-value strings.** Lists (`tags`) get comma-joined; nested values are flattened. Confirmed acceptable — keeps the `.cook` file portable.
- **Path-as-id and renames.** Meal files reference recipes by path. Renaming a recipe requires rewriting all referencing meals. Our existing vault rename pipeline already handles wikilink rewrites; extend to `Meal::recipe_paths`.
- **Cookware + timers.** Cooklang surfaces both. Expose in UI, ignore initially in `Recipe` struct beyond what cooklang gives us.
- **Nutrition.** Computed on demand from pantry's `nutrition_per_unit` × cooklang's scaled quantities. Partial if some ingredients aren't in pantry. Acceptable.
- **Name collisions.** Two pantry items named "butter" → first-match wins. Mitigation: pantry create-time uniqueness check (out of scope here).
- **`@ingredient` with no matching wiki/pantry page.** Renders as plain text. Pantry-fulfillment check flags as "not in pantry" as today. No new failure mode.
- **`features/inventory/` overlap.** Unchanged — pantry items still extend inventory items.
- **Substitutions ratio + cooklang units.** Confirm `cooklang::Converter` is sufficient for ratio math during sub application; may still need `pantry::units` as fallback.

---

## Out of scope (this migration)

- Replacing `pantry/` with `cookcli`'s `.pantry.conf` — ours is richer (per-batch stock entries, OpenFoodFacts, expiry, opened state, FIFO).
- LSP integration in our editor.
- `cooklang-import` URL→recipe LLM importer.
- Federation / sync over `cooklang-sync-client`.
- Multi-recipe menu file (`*.menu`) authoring — phase 5 evaluates whether to read them; authoring is later.

---

## Done = 

- All recipes in the test vault are single `.cook` files.
- `mealplan/tests/end_to_end.rs` passes against the new format.
- `cargo check -p mealplan` and `cargo check -p task-app-web --target wasm32-unknown-unknown` clean.
- Migration tool runs on `~/Development/Vaults/Observatory` and produces well-formed cooklang for every existing recipe.
- Updated [`mealplan-grocy-parity.md`](mealplan-grocy-parity.md) to note the format change.
