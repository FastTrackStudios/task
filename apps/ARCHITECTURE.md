# Architecture

What Task is made of and how a request flows through it. Kept short
and true; if it disagrees with the code, the code wins — fix this
file. Companion docs: [AGENTS.md](AGENTS.md) for working rules,
`docs/architecture/vault-crdt-reconciliation.md` for the collaborative
editing design.

## In one paragraph

Task is a Rust workspace inside the FastTrackStudio monorepo. Domain
logic lives in ~37 feature slices under `features/task/`, each
exposing an `#[architect::rpc]` service trait. `task-server` mounts
those services into one `architect::LayerRouter` **per organization**
and serves it over a WebSocket at `/org/{slug}/vox`. Clients — the
Dioxus app (web / desktop / mobile), the `task` CLI, and the watchOS
app via an HTTP bridge — are typed vox clients over that router.
Most entities are stored as **markdown files with YAML frontmatter**
in the org's vault; a minority live in per-service sea-orm databases;
Loro CRDTs are used only to make individual vault files
collaboratively editable in real time.

## Crate topology

```
apps/task/
  cli/       task-cli          → binary `task`
  server/    task-server       axum + LayerRouter, one router per org
  web/       task-app-web      wasm bundle (dx)
  desktop/   task-app-desktop
  mobile/    task-app-mobile
  watchos/   SwiftUI app over the server's /watch/v1 HTTP bridge
  ui-lab/    TS component lab (vite + pnpm)

crates/task/
  ui/            package `ui`  — the Dioxus app shell: router, pages,
                                 theming, stores, vox session, collab
  telemetry/     architect-telemetry — sentry + tracing layer
  xtask/         xtask          — TS codegen

features/task/<slice>/
  <slice>-proto  entities + #[architect::rpc] traits; under `vox`,
                 architect emits Client / Dispatcher / *_descriptor()
  <slice>        facade + backend impl
  <slice>-db     sea-orm entities + migrations   (only threads, finance)
  <slice>-ui     dumb Dioxus components          (only task, threads,
                                                  scheduling)
  <slice>-live   filesystem backend + watcher    (vault, wiki)
```

The 37 slices: agent, attachments, collection, contacts, cycle, email,
finance, fitness, git, goal, identity, inbox, inventory, label, links,
locations, mealplan, media, milestone, mount, org, prefs, project,
recall, resources, scheduling, scripture, share, tag, task, threads,
timer, vault, view, wiki, workflows, workstream.

The shape is **not** uniform. Some slices are proto-only
(`attachments`, `label`, `media`, `share`, `org`, `workflows`) with the
implementation in the server. Some have no proto crate at all
(`task`, `project`, `goal`, `cycle`) and declare their
`#[architect::rpc]` trait inside the facade — e.g.
`features/task/task/task/src/service.rs`, whose descriptor is reached
as `task::task_service_descriptor()`.

Framework crates live at `libs/architect/*` (in-tree, path deps) and
the design system at `libs/architect-ui/architect-ui`.

## Request path

```
client
  vox_core::initiator_on(link).establish::<SomeClient>()
        │
        │  WebSocket
        ▼
  /org/{slug}/vox      per-org lane      (apps/task/server/src/lib.rs)
  /server/vox          org lifecycle, identity, snapshot
  /vox                 legacy alias into the first hosted org
        │
        ▼
  architect::axum_ws::serve_router
        │
        ▼
  snapshot::GatedRouter        parks requests during snapshot/restore
        │
        ▼
  architect::LayerRouter       org_layer_router(&OrgAppState)
        │   one dispatcher per service descriptor:
        │     .with(<svc>_descriptor(), <Svc>Dispatcher::new(impl))
        │   auth is wrapped in AuthServerMiddleware; the permissions
        │   gate enforces capabilities when TASK_ENFORCE_PERMISSIONS=1
        ▼
  per-org service impl
        │
        ├──→ vault markdown  (the default; source of truth)
        ├──→ sea-orm         (per-service sqlite)
        └──→ Loro DocRegistry (collaborative file editing)
```

Every organization gets its own `OrgAppState` and its own
`LayerRouter`, so services are isolated per org by construction rather
than by a tenant column. Org data roots live at
`$TASK_DATA_ROOT/orgs/<slug>/` (default `$HOME/.task`), and an org is
portable: copy the directory, point a server at it.

### Non-vox HTTP surfaces

Deliberate exceptions, each because a client can't speak vox there:

| Route | Why |
|---|---|
| `/health`, `/org/{slug}/health` | liveness probes |
| `/.well-known/task-server.json` | discovery + per-service schema stamps |
| `/org/{slug}/mcp` | MCP tool surface for LLM agents (bearer `TASK_MCP_TOKEN`) |
| `/org/{slug}/share/{token}` | public share-link landing pages |
| `/watch/v1/*` | watchOS cannot hold a WebSocket (TN3135) |
| `/media/{*path}`, `/org/{slug}/media/{*path}` | `<audio>`/`<img>` need plain URLs |
| SPA fallback | serves a `dx` web bundle same-origin when `TASK_SERVER_WEB_DIR` is set |

The `/media` HTTP routes are a known deviation from "everything over
vox" — `features/task/media/media-proto` exists to replace them; see
`plans/media-over-vox.md`.

## Storage

### Markdown vault — the default

An entity is a `.md` file: YAML frontmatter for properties, the body
below. Identity is a UUID `id` in frontmatter, **not** the filename —
renames preserve it. Entities are discriminated by `type: <kind>` in
frontmatter or a matching entry in `tags`. Backends walk the vault per
call; any index is disposable and rebuildable from the files.

Vault-backed at the server: task, project, goal, milestone,
workstream, inbox, recall, contacts, tag, locations, inventory,
mealplan (cookbook / shopping / substitutions / pantry), fitness (body
/ exercises / workouts / intake), scheduling, wiki, and the vault
slice itself.

The vault is Obsidian-shaped on purpose — `vault-obsidian` reads and
writes the same directory an Obsidian client would, including
wikilinks, properties, and `.base` views. `vault-live::bases`
implements the `.base` YAML query DSL (`execute_view`) over generic
vault rows; that, plus per-service filters like
`task::TaskListFilter`, is the query layer. There is no generic
cross-entity query engine.

### sea-orm — per-service databases

A minority of slices are DB-backed. Each defaults to an org-local
sqlite file and takes a URL override:

| Slice | Env |
|---|---|
| agent task queue | `TASK_SERVER_AGENT_TASKS_URL` |
| timer | `TASK_SERVER_TIMER_URL` |
| threads | `TASK_SERVER_THREADS_URL` |
| prefs | `TASK_SERVER_PREFS_URL` |
| finance | `TASK_SERVER_FINANCE_URL` |

Plus architect's own schemas: auth (`architect-auth`), permissions,
and the share-link registry. Migrations run at boot; there is no
separate migrate/seed binary.

Caveat when reading Cargo.tomls: ~48 crates depend on sea-orm without
being DB-backed, because `#[derive(architect::Entity)]` emits a
sea-orm Model under a `server` feature. The dependency alone proves
nothing — check whether a consumer enables that feature.

### Loro CRDT — collaborative text editing only

`features/task/vault/vault-collab` runs one `VaultCollab` per org over
a `crdt::DocRegistry`. Doc id is UUIDv5 of `(vault_id, path)`; docs
persist per-doc under the org's `crdt/` root
(`TASK_SERVER_CRDT_ROOT`). Three loops: open/seed from the file,
debounced write-behind into the file, and inbound merge of external
file writes into open docs, with a sha-based echo guard. The client
half is `crates/task/ui/src/{collab,presence}.rs`.

The file remains the source of truth; the CRDT is a live editing layer
over it. **There is no entity CRDT layer** — `EntityCrdt`, `*RepoLoro`
and per-slice `-crdt` crates were removed
(`plans/done/project-crdt-rip.md`).

## Serialization

Domain types derive **Facet**, which gives YAML (frontmatter), JSON,
and the vox wire format from one definition, and is wasm-safe. `xtask
codegen` emits TypeScript clients from the same service descriptors
for ui-lab.

Because vox method ids hash the method name and payload shapes,
changing a proto changes the wire contract: a server built before the
change cannot talk to clients built after it. The server publishes
per-service schema stamps at `/.well-known/task-server.json` and
`task doctor` compares them against the local build.

## Clients

| Client | Transport |
|---|---|
| `task-app-web` | vox over WebSocket; URL baked at build time via `option_env!("TASK_VOX_URL_WEB")` (wasm can't read env at runtime) |
| `task-app-desktop`, `task-app-mobile` | vox over WebSocket; `TASK_VOX_URL` at runtime |
| `task-cli` | vox over WebSocket, or in-process services when `TASK_EMBED=1` |
| watchOS | HTTP bridge at `/watch/v1/*` |
| LLM agents | MCP at `/org/{slug}/mcp` |
| Obsidian | none needed — it edits the vault files directly |

All UI clients share one Dioxus crate, `crates/task/ui`; the app crates
are thin platform shells over it.

## Deployment

`nix build .#task-server` builds the server; `.#task-server-image`
streams a container image. The live path is the Helm chart at
`apps/task/deploy/chart/` (server + web + ui-lab deployments, ingress,
PVC, backup script), with `docs/starcommand-webapp-runbook.md` as the
operator runbook. `.env.example` is the complete env-var inventory.

`apps/task/nix/module.nix` is an orphan NixOS module that nothing in
the flake imports, and it sets env vars the server no longer reads
(`TASK_VAULT`, `TASK_DB_PATH`, `TASK_SEED_DEMO`, `NEXTCLOUD_*`). Don't
use it as a reference.
