# MVP — todos live on the web + knowledge core wired end-to-end

**Status:** active. Top of stack. Everything else in `plans/` queues
behind this.

## Goal

Daily-drivable MVP, deployed to the k8s cluster (deployment itself is
a later step — this plan only keeps it *deployable*):

1. Track todos + projects in the web UI.
2. LLM wiki injection working (`task wiki context` feeding agents).
3. Obsidian-compatible linked notes — `[[wikilinks]]`, backlinks, a
   real vault on disk.
4. Everything on the architect + architect-atom stack — one pattern
   for services, entities, and client state.

Once live and in daily use, additions resume (Research/Federation,
mealplan chain, email entity, SSR, HA).

## Already true (verified 2026-07-01)

- **UI state**: 100% on architect atom stores
  ([atom-store-migration.md](atom-store-migration.md), shipped).
- **Services**: every feature is `#[architect::rpc]` → mounted on
  `/org/<slug>/vox` in `apps/server/src/lib.rs`; web + CLI share the
  generated clients (`crates/ui/src/vox_clients.rs`).
- **Todos/projects on web**: TaskService + ProjectService live.
- **Deployable**: `deploy/chart/task` Helm chart (server+web
  Deployments, PVC `/data`, Ingress, probes, git-snapshot CronJob);
  images from `flake.nix` via `dockerTools.streamLayeredImage`.
  Single sqlite writer → 1 server replica, Recreate strategy. Fine.
- **Vault**: read/write, wikilinks + aliases + anchors, `.obsidian/`
  honored; `VaultGraph` RPC (backlinks/links/orphans/unresolved/
  deadends/tags) mounted, backs the vault page's backlinks panel.
- **Typed links**: `LinksService` mounted; `apps/server/src/link_sync.rs`
  auto-syncs note→**verse** wikilinks into the store on save.
- **Wiki**: ingest queue, index/log, 4-signal graph, gaps, lint,
  review, token-budgeted `wiki_graph::build_context` (CLI
  `task wiki context`).

## Remaining work (order = priority)

### 1. note→note wikilink sync

`link_sync.rs` skips any wikilink that isn't a verse ref, so the
/connections graph (built from the typed-link store) has no
note↔note edges — the "Obsidian graph" is missing its main edge type.

- Extend `sync_note`: targets that fail `VerseRange::parse` become
  `TypedLink { Note → Note, Relation::Mentions }` (same relation the
  verse sync uses; there is no dedicated `Wikilink` relation).
- Resolve targets through `vault::GraphBackend::links("default", path)`
  (same resolver the backlinks panel uses — exact path, basename,
  aliases) instead of growing the copied regex. Unresolved targets are
  skipped (they'll sync when the target page appears; page saves
  re-trigger).
- Same provenance discipline: `source_ref = "vault-link-sync"`,
  `derived = true`, `Visibility::Private`; re-sync replaces only its
  own links.
- Full-vault `LinkIndex::build` per save is O(vault) — fine at MVP
  scale. `// FUTURE:` incremental index if vaults grow.

### 2. Unify LLM context: vault notes + typed links into wiki-graph

`build_context` only walks `<vault>/Wiki/`. Prose notes and
typed-link edges are invisible to injection.

- Fold typed-link store edges (`links.jsonl`) into the wiki-graph
  edge set (a 5th signal, or map onto "direct link" ×3).
- Include vault note nodes reachable from the focal set so a context
  budget can pull in the user's own notes, not just ingested wiki
  pages.
- Surface via CLI (`task wiki context --include-notes`?) and the
  /connections focal subgraph.

### 3. Entity consistency — ✅ done (audited 2026-07-01)

[architect-entity-followups.md](architect-entity-followups.md) was
stale: task already has its Uuid PK, cookbook/pantry/mealplan/wiki/
scheduling/vault-proto are all Entities, String PKs are proven
in-tree, mealplan tests pass. Only the deliberate skips remain
(git-proto wire-only, email-config TOML). Nothing to do.

### 4. Deployability guard

No deploy yet — just keep it green:

- `nix build .#task-server-image .#task-web-image` + `helm lint
  deploy/chart/task` stay passing (add to pre-push habit or CI).

## Deferred (post-MVP)

Wiki Research / Federation / Events impls · mealplan entity chain
(fix its pre-existing tests first) · email-config entity · SSR
([web-ssr-investigation.md](web-ssr-investigation.md)) · HA /
metrics / OIDC · git-proto + attachments stay wire-only (not debt).

## Acceptance

- Create/complete a todo in the web UI against the server; survives
  server restart.
- Save a note with `[[Another Note]]` and `[[John 3:16]]`; both edges
  appear in /connections; backlinks panel shows the reverse.
- `task wiki context <query>` returns a token-budgeted subgraph that
  can include prose notes linked to the topic.
- `nix build` images + `helm lint` pass.
- `cargo check -p task-ui` + `cargo check -p task-app-web --target
  wasm32-unknown-unknown` clean.
