# ui-lab

A Vite + React + TypeScript design lab for prototyping Task's UI
against the **real running task-server**, over vox's TypeScript stack —
with visual parity to the Dioxus app (same FTS token sheet).

This is **not** the product UI. It exists so design iteration can move
at browser-hot-reload speed before a screen is ported to Dioxus.

## The prototype→port loop (policy)

1. **One route at a time.** A prototype claims one lab route
   (`/projects`, `/projects/$id`, ...) and iterates there.
2. **Fetch + render only.** Prototypes read through the generated vox
   clients and render. No mutations, no local persistence, no state
   machinery beyond TanStack Query. If a prototype needs a write path,
   that's a sign it's done prototyping.
3. **Throwaway by policy.** Nothing in `src/routes/` is product code.
   It is allowed to be ugly inside, fast to change, and deleted without
   ceremony.
4. **Port on approval, then freeze.** When a design is approved it gets
   ported to the Dioxus app (`crates/ui` / `features/*-ui`). The lab
   route is then frozen — kept only as the reference rendering until
   the port ships, after which it can be deleted.

## Running

The task-server must be up on `127.0.0.1:18080` hosting the home org
(slug `codywright`):

```sh
pnpm install
pnpm dev        # http://localhost:5173 — /projects, /projects/<uuid>
pnpm build      # tsc -b && vite build
pnpm smoke      # wire-level check against the live server, from node
```

The browser app opens its vox WebSocket **same-origin** and vite
proxies `/org/**` to the server (see `vite.config.ts`). Chrome's Local
Network Access checks can stall cross-origin `ws://` to loopback, and
same-origin is what the production mount looks like anyway. Override
the proxy target with `TASK_SERVER_HTTP=http://host:port pnpm dev`;
override the org with `VITE_TASK_ORG=<slug>`. `pnpm smoke` (node, no
proxy) dials `ws://127.0.0.1:18080` directly, or honors
`VITE_TASK_SERVER` / `VITE_TASK_ORG`.

## How the vox clients are made

```
#[architect::rpc] trait  →  <name>_service_descriptor()  →  vox-codegen  →  src/generated/*.generated.ts
   (features/*/src)            (vox feature flag)            (xtask)
```

- Each `#[architect::rpc]` service trait in the workspace emits a
  `<snake_name>_service_descriptor()` returning a
  `&'static vox_types::ServiceDescriptor` (enabled by the proto crate's
  `vox` feature).
- `cargo xtask codegen` (repo root) feeds those descriptors to
  `vox_codegen::targets::typescript::generate_service` and writes
  `ui-lab/src/generated/<service>.generated.ts` — full typed client,
  method ids, and the postcard send-schema tables for the wire
  handshake. To add a service, add its descriptor to
  `service_descriptors()` in `xtask/src/main.rs` and rerun.
- The generated code runs on `@bearcove/vox-core` / `@bearcove/vox-ws`,
  vendored in `vendor/` and pinned to the exact vox rev in `Cargo.lock`
  so client and server protocol can't drift. See `vendor/VENDOR.md`.

Type caveats (current codegen):

- Rust `Uuid` / `DateTime` cross the wire as strings but are typed
  `unknown` in TS — `String(p.id)` is the pattern.
- Rust `i64`/`u64` decode as `bigint`; `JSON.stringify` throws on them —
  use `dump()` from `src/lib/utils.ts`.

## Theme parity

The lab no longer vendors `src/styles/fts-theme.css`. That copy had
gone stale: it still carried the pre-2026-07-03 shadcn dark palette, so
it never picked up the Obsidian dark colours the Dioxus app switched
to — and `src/styles/index.css` had already stopped importing it ("the
signal-* tokens it defined aren't needed in the lab"). It was dead,
drifted weight, so it has been deleted.

The lab now takes its CSS variables from the `:root` / `.dark` blocks
`shadcn apply` writes into `src/styles/index.css` directly. The Dioxus
app's tokens live at `libs/architect-ui/architect-ui/assets/fts-theme.css` (the one
canonical sheet in the tree) with Task's palette overrides layered on
top in `apps/task/fts-theme.css`; if you want the lab to track the app
exactly, `@import` those rather than re-copying them. The shadcn/ui primitives
in `src/components/ui/` (button, card, badge, skeleton, tabs) are
styled entirely by those tokens, so the lab and the Dioxus app render
from the same palette. `components.json` is set up for
`pnpm dlx shadcn@latest add <component>` when more primitives are
needed.

## Lab routes

- `/projects` — real projects from the home org (title, status, kind)
  via `ProjectServiceRpc.list`; skeleton + error/retry states.
- `/projects/$projectId` — one project (`get`) plus its tasks
  (`TaskServiceRpc.list`, filtered client-side on `project_id` — the
  task service intentionally has no server-side filters yet), with a
  Raw tab dumping both payloads. This is the canvas the project-detail
  redesign builds on.

Known live-data quirks: a vault page whose frontmatter has no persisted
`id:` gets a *fresh* backfilled UUID on every scan, so `get(id)` after
`list()` can be NotFound for that page (the smoke script skips those);
and tasks only attach to a project once their `projectId` frontmatter
is set.
