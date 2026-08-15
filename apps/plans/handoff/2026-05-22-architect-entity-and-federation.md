# Handoff — architect::Entity migration + federation Phase 1

Session date: 2026-05-22 → 2026-05-23
Branch: `main` (clean; cargo.lock dirty from formatter, restore with `git checkout -- Cargo.lock`)

## What landed (18 PRs in Task, 1 in architect)

### Federation Phase 1 (DONE end-to-end)
- **#64** — `OrgManifest` (architect::Entity, file-backed TOML) + `DataRoot` / `OrgRoot` resolvers + `task org init|list` CLI
- **#65** — CLI auth/timer/finance routed through `OrgRoot`; `CliSession` grows to multi-server shape; legacy `{token, user_id, email, org_id}` upgrades on first read
- **#66** — Server-side `OrgRoot` resolvers (single-org boot, paths through OrgRoot)
- **#75** — **Server multi-org routing.** Scans `<data_root>/orgs/` at boot, hosts every org. New routes: `/.well-known/task-server.json`, `/org/<slug>/health`, `/org/<slug>/vox`. Legacy `/vox` + `/health` kept as back-compat. Smoke-verified with two orgs.

### architect::Entity migrations (12 features)
- **#67** project (`ProjectInfo`)
- **#68** body (`BodyMetric`)
- **#69** exercises (`Exercise`)
- **#70** intake (`IntakeLog`) — DailyTarget wrapper still in place; can simplify now that cookbook::Nutrition is JsonField
- **#71** workouts (`Routine` + `WorkoutSession`)
- **#72** inventory (`Item`)
- **#73** locations (`Location`)
- **#76** task (`TaskInfo` + `TimeEntry`) — added `id: Uuid` PK with `Uuid::new_v5(NAMESPACE_URL, path)` backfill
- **#77** vault-proto (`ManifestEntry`) — **first String PK**, proved architect supports String primary keys
- **#78** cookbook (`Recipe`) — String PK on `path`; `Nutrition` derives `JsonField` so downstream crates use it directly
- **#79** pantry (`PantryItem`) — biggest single-crate migration; 30 fields, cross-crate `Nutrition`/`Item` refs, three nested-collection newtypes
- **#80** mealplan (`Meal` + `ShoppingList` + `SubstitutionRule`) — three entities in one PR

### Architect repo
- **#2** (codywright/architect) — `fix(derive): strip serde/schemars from forwarded field attrs` — unblocked every entity that doubles as a markdown/JSON wire format. Without this, `#[serde(rename = "camelCase")]` etc. leak onto the synthetic `Create` struct and rustc errors "cannot find attribute `serde` in this scope".

### Docs
- **#74** `plans/architect-entity-followups.md` — captures blockers + design decisions for the remaining crates
- `plans/federated-task-platform.md` is the source of truth for the federation architecture

## What's pending

### architect::Entity — 4 crates left

1. **wiki-proto** (highest value remaining)
   - 6 types: `WikiIndex`, `IndexSection`, `IndexEntry`, `LogEntry`, `ResearchPlan`, `PeerWiki`
   - All use `pub id: String` — String PK works (vault-proto / cookbook proved it)
   - Should be a smooth migration following the established pattern
   - Files: `features/wiki/wiki-proto/src/{log,research,federation,graph,review,ingest}.rs`
   - Many of these are pre-defined for federated wiki resolution per `plans/federated-task-platform.md`'s wiki section — getting them DB-mountable now means Phase 3 wiki work just plugs in

2. **scheduling-proto** — 3 types (`Booking`, `AvailabilitySchedule`, `TimeBlock`), all String IDs

3. **git-proto** — **defer/skip**. `Issue` / `Comment` mirror GitHub's API shape; not first-party entities. Per the followups doc, classify as wire-only and don't migrate.

4. **email-config** — defer. `AccountConfig` has `AccountId(String)` PK + a complex tagged `BackendKind` enum with `Secret` variants. Migration requires choosing how to JsonField the enum and how to handle secrets. Not blocking any feature today.

5. **attachments-proto** — already classified wire-only; no migration needed.

### Federation Phase 2+ (not started)

- **Phase 2**: project mounts — `Mount` entity + `~/.config/task/mounts.toml` + `ContentBackend` trait (filesystem / nextcloud / vox-proxy). See `plans/federated-task-platform.md` "On-disk layout" → "Per-machine mount registry".
- **Phase 3a**: identity links — `identity.sqlite` in home org, encrypted remote tokens, `task auth link <server-url>`. Federation handshake via home pubkey discovery.
- **Phase 3b**: federated wiki — `wiki-index.json` wire format + per-machine SQLite FTS5 cache, `[[@org/Page]]` cross-org syntax, fetch-on-demand body cache at `~/.cache/task/wiki/`.
- **Phase 4**: Nextcloud `ContentBackend` impl.

## Pattern reference (use this for the remaining migrations)

### Cargo.toml additions

```toml
[dependencies]
architect = { workspace = true }
serde_json = { workspace = true }   # JsonField round-trip
# ... existing ...

vox = { workspace = true, optional = true }
vox-types = { workspace = true, optional = true }
sea-orm = { workspace = true, optional = true }
fake = { version = "4", optional = true, features = ["derive", "chrono", "uuid"] }

[features]
default = ["vox"]
vox = ["dep:vox", "dep:vox-types", "architect/vox"]
server = ["architect/server-seaorm", "dep:sea-orm"]
fake = ["dep:fake", "architect/fake"]
full = ["vox", "server", "fake"]
```

If the crate depends on another architect-derived crate, cascade `server`/`fake` (see pantry's Cargo.toml: `server = [..., "cookbook/server", "inventory/server"]`).

### Entity skeleton

```rust
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "things", repo)]
pub struct Thing {
    // Uuid PK
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    // OR String PK (vault-proto, cookbook — works fine!)
    // #[architect(primary_key, auto_increment = false)]
    // pub path: String,

    #[architect(filterable, sortable, fulltext)]
    pub name: String,
}
```

### Vec<T> newtype pattern

Architect requires Vec fields to be wrapped (orphan rule on `From<Vec<T>> for sea_orm::Value`):

```rust
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct StringList(pub Vec<String>);

impl StringList {
    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

impl From<Vec<String>> for StringList { fn from(v: Vec<String>) -> Self { Self(v) } }
impl FromIterator<String> for StringList {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}
impl std::ops::Deref for StringList {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl std::ops::DerefMut for StringList {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}
```

Use as: `#[architect(json)] pub tags: StringList`.

For nested struct vecs (e.g. `Vec<BodyEntry>`): same pattern but the inner type also needs `#[cfg_attr(feature = "fake", derive(::fake::Dummy))]` so `Dummy` cascades.

### lib.rs crate attribute

Architect emits cfg-gated blocks; every migrated crate needs:

```rust
// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]
```

at the top of `src/lib.rs`.

### Common pitfalls hit this session

- `for x in &collection` doesn't work on the newtype because `&Newtype` doesn't impl `IntoIterator`. Use `collection.iter()` instead. (Deref auto-coerces method calls but not desugarings.)
- `task.field` access from cross-crate uses needs `.0` to unwrap the newtype (e.g. `pantry.tags.0.clone()`).
- When a downstream crate uses a type whose `fake::Dummy` lives in an upstream crate, the downstream's `fake` feature must enable the upstream's: `fake = ["dep:fake", "architect/fake", "cookbook/fake"]`.
- When a downstream uses a type whose SeaORM impls (`JsonField`-emitted `Nullable`/`TryGetable`/etc.) live upstream, the downstream's `server` feature must enable the upstream's: `server = [..., "cookbook/server"]`.

## Workflow notes

### Git / PR
- Working remote is `forgejo-https` (HTTPS). `origin` is SSH and currently has auth problems — use `forgejo-https` for fetch/push.
- Token at `~/.config/forgejo/token`, helper at `~/.local/bin/forgejo-token`.
- PR creation via `gh api`-equivalent forgejo curl. Pattern is captured throughout this session's bash commands.
- `capn` pre-push runs `cargo clippy --workspace --all-targets --all-features -- -D warnings`. This is strict — any warning fails the push.
- **`.git/index.lock` issues** kept hitting because capn auto-formats + restages large file sets concurrently with manual commands. Recipe when stuck:
  1. `rm -f .git/index.lock`
  2. `git stash`
  3. `git checkout main`
  4. `git merge --ff-only forgejo-https/main`
  5. `git stash pop` (or `drop` if not needed)
  6. `git checkout -- Cargo.lock`

### Capn auto-format
Capn touches Cargo.lock + reformats many files on push. After every successful merge:
```
git checkout -- Cargo.lock
```
to restore the canonical version.

### Architect cross-repo work
`architect` is path-dep'd at `../architect/macros/architect`. Edits there take immediate effect in Task. The session pushed one fix as `https://git.starcommand.live/codywright/architect/pulls/2` against main.

## Quick-start for the next session

```bash
cd /home/cody/Development/Task
git status                              # should be clean (or just Cargo.lock dirty)
git fetch forgejo-https
git merge --ff-only forgejo-https/main

# Pick a remaining migration target — wiki-proto is highest-value
git checkout -b feat/architect-entity-wiki-proto

# Pattern: edit Cargo.toml + 6 model files following the template above.
# Each WikiIndex/IndexSection/etc. just needs:
#   - architect::Entity derive
#   - String PK on `id`
#   - JsonField newtypes for any Vec<T>
#   - crate-level #![allow(unexpected_cfgs)] in lib.rs

# Verify
nix develop --command bash -c 'cargo check -p wiki-proto --all-features'
nix develop --command bash -c 'cargo clippy -p wiki-proto --all-targets -- -D warnings'

# Commit + push + PR + merge using the established curl pattern
```

## State verification

```bash
# Multi-org server still works
TMP=$(mktemp -d)
TASK_DATA_ROOT=$TMP/data ./target/debug/task org init alpha --name "Alpha Co"
TASK_DATA_ROOT=$TMP/data ./target/debug/task org init beta --name "Beta Inc" --home
TASK_SERVER_BIND=127.0.0.1:19090 TASK_DATA_ROOT=$TMP/data ./target/debug/task-server &
sleep 2
curl -s http://127.0.0.1:19090/.well-known/task-server.json | python3 -m json.tool
# Should list both orgs with vox + health URLs
```

## Things worth remembering

- **String PKs work in architect.** Confirmed in vault-proto, cookbook, mealplan SubstitutionRule. Don't waste cycles on a Uuid migration for entities that have a natural String identity (vault paths, slugs, etc.).
- **architect's `unexpected_cfgs`** warning about `feature = "vox"` is harmless — the gated code never compiles unless the host crate exposes a vox feature. Crate-level `#![allow(unexpected_cfgs)]` is the right answer.
- **Don't drop `#[serde(...)]` field attrs.** Architect (post-PR #2) correctly filters them from the synthetic Create struct. They stay on the wire struct, where serde derives pick them up for JSON/TOML/YAML round-trip.
- **The `feature = "fake"` cascade matters** for downstream crates. Skip it (set `fake = []` or omit) and you can't pass `--all-features`. Either cascade properly OR remove the optional dep + feature entirely.
- **Mealplan crates had pre-existing test failures** before this session (cookbook Recipe schema drift in `tests/end_to_end.rs` and `fulfillment.rs`). All fixed incidentally during the cookbook/pantry/mealplan migrations.
