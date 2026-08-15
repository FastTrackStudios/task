# architect::Entity migration — remaining work

**Status: complete (audited 2026-07-01).** Every "blocked on design" item below has since shipped: `task::TaskInfo` carries `#[architect(primary_key, on_create = Uuid::new_v4())] id: Uuid`; `cookbook::Recipe`, `pantry::PantryItem`, `mealplan::{Meal, ShoppingList, SubstitutionRule}` are Entities (mealplan tests pass); wiki (`IndexEntry` path-PK, `LogEntry`, `PeerWiki`), scheduling-proto (booking/schedule/time_block/day_plan/event_type/cal_event), and `vault-proto::manifest` are Entities — String PKs confirmed supported in-tree. Deliberate non-migrations stand: git-proto (wire mirror of external forge), email-config (TOML + keyring, defer to federation), attachments-proto (RPC-only). The sections below are kept as the historical design record.

Tracks the crates still to migrate after PRs #67–#73. Each is blocked on a design call that needs user input rather than a mechanical pattern application.

## Done (PRs #67–#73)

| Feature | Crate | Status |
|---|---|---|
| project | `project` | ✅ #67 |
| body | `body` | ✅ #68 |
| exercises | `exercises` | ✅ #69 |
| intake | `intake` | ✅ #70 |
| workouts | `workouts` | ✅ #71 |
| inventory | `inventory` | ✅ #72 |
| locations | `locations` | ✅ #73 |
| **org** | `org-proto` | ✅ #64 (already done) |
| **finance / timer / agent** | various | ✅ (pre-existing) |

## Blocked on design

### `task::TaskInfo` / `task::TimeEntry`

**Blocker:** No stable `id` field today. Tasks are identified by their vault path.

**Options:**
- (a) Add `id: Uuid` to `TaskInfo` with auto-backfill on parse (parser generates new id when frontmatter lacks one, writer persists it). Same pattern as `ProjectInfo`. Introduces a `id:` line to every existing task page on first re-save.
- (b) Use the vault path as the primary key (architect supports `String` PKs). Avoids the backfill but breaks if a task is renamed.
- (c) Keep `task` outside the architect::Entity world for now — it's the most user-touched entity and getting it wrong has high cost.

**Lean:** (a). Path-as-PK loses the renaming invariant; (c) leaves a hole in the schema-first vision.

### `mealplan/cookbook::Recipe`

**Blocker:** No `id` field. Recipes are `.cook` files identified by path.

**Options:** same as task. Plus `Recipe` has nested `Ingredient` / `Nutrition` (already a value type used by intake's `DailyTarget`); migrating `Nutrition` to `architect::JsonField` would let `intake::DailyTarget` re-enable its `fake` feature.

### `mealplan/pantry::PantryItem`

**Blocker:** Has `Uuid id` so the entity derive itself is straightforward, but:
- Carries `Option<Nutrition>` (cross-crate, see cookbook above)
- 4× `Vec` fields (`tags`, `stock_entries`, `substitutes`, `barcodes`)
- `StockEntry` + `Substitution` are themselves persisted-shape types in the audit

**Path:** Migrate after cookbook so `Nutrition` is already JsonField-friendly. Then PantryItem follows the body/intake pattern (one Entity, multiple JsonField newtypes).

### `mealplan/mealplan::Meal` / `ShoppingList` / `SubstitutionRule`

**Blocker:** Has Uuid ids on the top-level entities, BUT the crate has **pre-existing test failures** (cookbook `Recipe` schema drifted from the tests in `tests/end_to_end.rs` and `src/fulfillment.rs`). Those failures must be fixed before adding more changes.

### `wiki/wiki-proto::WikiIndex` etc.

**Blocker:** Every type uses `pub id: String` (not Uuid). 6 types: `WikiIndex`, `IndexSection`, `IndexEntry`, `LogEntry`, `ResearchPlan`, `PeerWiki`.

**Options:**
- Switch all ids to Uuid (breaking change to existing wiki indexes on disk)
- Verify architect supports `String` primary keys + use them as-is

**Lean:** Verify String PK support first, keep wire format stable.

### `scheduling/scheduling-proto::Booking` / `AvailabilitySchedule` / `TimeBlock`

**Blocker:** Same as wiki — `BookingId(String)`, `TimeBlockId(String)`, etc.

### `git/git-proto::Issue` / `Comment`

**Blocker:** These mirror GitHub's API shape (`IssueId(u64)`, `Comment.id: String`). They are wire mirrors of an external service, not first-party entities. Question for the user: should these EVEN be `architect::Entity` (we don't own the schema)?

**Lean:** Skip. Mark as "wire-only" and stop tracking them in the migration audit.

### `email/email-config::AccountConfig` / `SmtpConfig`

**Blocker:**
- `AccountId(String)` PK — same string-id question
- `BackendKind` is a tagged enum with variants carrying data (IMAP host, port, OAuth credentials, …) — architect's `json` attribute on the enum field is the path, but the enum needs `JsonField` and careful handling of `Secret` (which already has its own keyring abstraction)

**Lean:** Defer until the federation work needs email-config DB-mounted. Today it's a TOML config and that's fine.

### `vault-proto::Manifest` / `ManifestEntry`

**Blocker:** No id field (it's a *manifest* — a list of files by path). The natural PK is path. `Manifest` is a container; entries are the "rows".

**Path:** `ManifestEntry` as an Entity with `path: String` as PK. `Manifest` stays as a wire-only container (just a `Vec<ManifestEntry>`).

### `attachments-proto`

**Already wire-only.** `AttachmentMeta`, `InitiateUpload`, `UploadTicket`, etc. are RPC types, not persisted entities. No migration needed.

## Path forward

1. **String PK support check** — ✅ answered in-tree (2026-07-01): `wiki-proto::IndexEntry` uses `#[architect(primary_key, auto_increment = false)] pub path: String` and `PeerWiki.id: String`; both compile + serve. Wiki/scheduling/vault-proto can keep String ids as-is.
2. **Cookbook first** — migrate `Recipe` / `Ingredient` / `Nutrition` (with new `id: Uuid` on Recipe). Unblocks pantry + intake.
3. **Pantry** — follows immediately.
4. **Task** — separate PR; adds `id: Uuid` with backfill.
5. **Mealplan** — fix pre-existing test failures first, then migrate.
6. **Wiki / scheduling / vault-proto** — depend on String PK answer.
7. **Git-proto / email-config** — likely skipped or deferred.

Federation stack work can proceed in parallel; the entity migration doesn't block it.
