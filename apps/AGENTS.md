# Agent Guide

Canonical instructions for AI agents working on **Task** — the
local-first, collaborative work-management product that lives inside
the FastTrackStudio monorepo. Read this first.

Task is Rust + Dioxus on the **architect** framework, with a
**markdown vault** as its primary store. Read `README.md` for the
product story and `VISION.md` for where it's going; this document
covers the *how*, and the repo-root `CLAUDE.md` covers monorepo-wide
rules that also apply here.

---

## TL;DR — the first 60 seconds

1. **Read the repo-root `CLAUDE.md`.** Task is not a standalone repo:
   ONE root Cargo workspace, path deps only, one `target/`, one flake.
2. **Skim `plans/`** — top-level files are active or reference work,
   and every one carries a status header. `plans/done/` is the
   terminal directory; do NOT read it to learn what Task is today.
3. **Survey `features/task/`** before adding anything. 37 slices
   already exist.
4. **Match the nearest slice** — but read it first. The slice shapes
   are *not* uniform (see below); there is no single template.
5. **Verify with `cargo check`**, native and wasm, before claiming
   done.

---

## Layout

Task's code is spread across three top-level directories of the
monorepo. There is no `apps/db`, no `task-db` crate, no
`crates/task-ui`, and no `crates/task-core`.

```
apps/task/
  cli/          package `task-cli` → binary `task`
  server/       package `task-server` — axum + architect LayerRouter
  web/          package `task-app-web` — dx web build (wasm)
  desktop/      package `task-app-desktop`
  mobile/       package `task-app-mobile` (+ ios/)
  watchos/      SwiftUI watch app over the server's /watch/v1 bridge
  ui-lab/       TS/vite component lab (pnpm)
  deploy/       helm chart + docker compose
  tests/        playwright/ (smoke) and multiplayer/ (conformance)
  plans/ docs/ skills/ hooks/ nix/ scripts/

crates/task/
  ui/           package `ui` — THE Dioxus app shell: router, pages/,
                theming, stores, vox session. NOT `task-ui`.
  telemetry/    package `architect-telemetry` — sentry + tracing layer
  xtask/        package `xtask` — TS codegen (see "Known stale" below)

features/task/<slice>/
  37 slices: agent attachments collection contacts cycle email finance
  fitness git goal identity inbox inventory label links locations
  mealplan media milestone mount org prefs project recall resources
  scheduling scripture share tag task threads timer vault view wiki
  workflows workstream
```

**Naming trap**: `task-ui` is `features/task/task/task-ui` — the
*task-list* component crate. The application shell is `crates/task/ui`,
whose package name is plain `ui`. `cargo check -p ui` and
`cargo check -p task-ui` are different things.

## Slice shapes (there is more than one)

A slice is a directory under `features/task/`. Crate roles, by how
often they actually appear:

| Role | Contains | Example |
|---|---|---|
| `<slice>-proto` | Wire contract: entity types + `#[architect::rpc]` traits. Under the `vox` feature architect emits the client, dispatcher, and `*_descriptor()`. | `timer-proto`, `vault-proto` |
| `<slice>` | Facade + backend implementation. Re-exports the proto's public surface behind feature gates. | `timer`, `contacts` |
| `<slice>-db` | sea-orm entities + migrations, when the slice is DB-backed. **Rare** — only `threads-db` and `finance-db` exist. | `threads-db` |
| `<slice>-ui` | Dumb Dioxus components. **Rare** — `task-ui`, `threads-ui`, `scheduling-ui`. | `threads-ui` |
| `<slice>-live` | Live filesystem/watcher backend. | `vault-live`, `wiki-live` |

Real variations you will hit, all of them legitimate:

- **Proto only** — `attachments`, `label`, `media`, `share`, `org`,
  `workflows`. The impl lives in the server or a sibling.
- **No proto at all** — `task`, `project`, `goal`, `cycle`. The
  `#[architect::rpc]` trait lives *inside* the facade crate (e.g.
  `features/task/task/task/src/service.rs`). `xtask` calls
  `task::task_service_descriptor()` and
  `project::project_service_descriptor()` directly.
- **Many crates** — `email` (13), `wiki` (7), `agent` (7), `vault` (6),
  `fitness` (6), `view` (7).

**Do not invent a shape.** Copy whichever neighbouring slice is
closest to what you're building, and say so in the commit message.

---

## Where data actually lives

Three stores. Knowing which one a slice uses is the single most
important thing to establish before you write code.

### 1. The markdown vault — the default, and the source of truth

Most slices store entities as **markdown files with YAML frontmatter**
under the org's vault root (`$TASK_DATA_ROOT/orgs/<slug>/vault`,
overridable with `TASK_SERVER_VAULT_ROOT`). The backend walks the
vault on each call; the file is authoritative and any index is
disposable.

Vault-backed at the server (`apps/task/server/src/lib.rs`): `task`,
`project`, `goal`, `milestone`, `workstream`, `inbox`, `recall`,
`contacts`, `tag`, `locations`, `inventory`, `mealplan` (cookbook /
shopping / substitutions / pantry), `fitness` (body / exercises /
workouts / intake), `scheduling`, `wiki`, plus the `vault` slice
itself.

Entity discrimination is by frontmatter: `type: task` **or** a `task`
entry in `tags`. Entities carry a UUID `id` in frontmatter — **the
filename is not the identifier**, and rename preserves `id`.

### 2. sea-orm — server-private and per-service tables

A minority of slices are database-backed. Their DB URL is
configurable per service and defaults to an org-local sqlite file:

| Slice | Env override |
|---|---|
| agent task queue | `TASK_SERVER_AGENT_TASKS_URL` |
| timer | `TASK_SERVER_TIMER_URL` |
| threads | `TASK_SERVER_THREADS_URL` |
| prefs | `TASK_SERVER_PREFS_URL` |
| finance | `TASK_SERVER_FINANCE_URL` |

Plus architect's own schemas: auth, permissions, and the share-link
registry.

Note that ~48 crates *depend on* sea-orm without being DB-backed:
`#[derive(architect::Entity)]` emits a sea-orm Model under a `server`
feature, and the dep comes along for the ride. Depending on sea-orm
proves nothing — check whether anything enables that feature. (For
`task`, nothing does; every consumer takes `features = ["vox"]`.)

### 3. Loro CRDT — collaborative text editing of vault files, only

Loro is used in exactly one place: real-time collaborative editing of
markdown files in the vault.

- Server: `features/task/vault/vault-collab` — one `VaultCollab` per
  org over a `crdt::DocRegistry`; doc id = UUIDv5 of `(vault_id,
  path)`; docs persist under the org's `crdt/` root
  (`TASK_SERVER_CRDT_ROOT`).
- Client: `crates/task/ui/src/{collab,presence}.rs`,
  `pages/note_view.rs`.
- Design doc: `docs/architecture/vault-crdt-reconciliation.md`.

The write-behind loop projects doc text back into the vault file, so
the **file stays the source of truth**; the CRDT is a live editing
layer over it, not a replacement.

**There is no entity CRDT layer.** `EntityCrdt`, `*RepoLoro`, and
per-slice `-crdt` crates do not exist — that architecture was removed
(`plans/done/project-crdt-rip.md`, `plans/done/knowledge-rip.md`).
Any doc telling you "Loro is the source of truth, never write to
SeaORM" is describing a design that was ripped out. Ignore it and fix
it.

---

## The request path

```
client (web / desktop / mobile / CLI)
  │  vox_core::initiator_on(link).establish::<Client>()
  ▼
WebSocket  /org/{slug}/vox        (also /server/vox, and /vox as a
  │                                legacy alias into the first org)
  ▼
architect::axum_ws::serve_router
  ▼
snapshot::GatedRouter  ── parks requests during snapshot/restore
  ▼
architect::LayerRouter  ── org_layer_router(&OrgAppState)
  │  one dispatcher per service descriptor:
  │    .with(<slice>_proto::<svc>_descriptor(), <Svc>Dispatcher::new(impl))
  ▼
per-org service impl
  ▼
vault markdown  ·  sea-orm  ·  Loro doc registry
```

Every org gets its own `OrgAppState` + `LayerRouter`. Server-management
services (org lifecycle, identity, snapshot) live on a separate
`/server/vox` router. Non-vox HTTP surfaces are deliberate exceptions:
`/health`, `/.well-known/task-server.json`, `/org/{slug}/mcp`,
`/org/{slug}/share/{token}`, `/watch/v1/*`, the `/media` file routes,
and `/org/{slug}/dav[/…]` (the Files WebDAV bridge — WebDAV clients
speak plain HTTP, so identity cannot ride the WS upgrade; see
`docs/files-webdav.md`).

---

## Hard rules

### RPC / services

1. **Services are `#[architect::rpc]` traits.** Not `#[vox::service]`
   — that macro is still used elsewhere in the monorepo (signal,
   input) but nothing in the Task tree uses it. Max 4 params per
   method (Facet constraint).

2. **`#[subscribe]` streams are the target idiom; most services have
   not migrated.** Today only `task` and `workstream` use
   `#[subscribe]` (with a `PubSub` fan-out hub in `backend.rs`); about
   a dozen crates still use the older `tx: Tx<T>` parameter form.
   **This is a migration in progress.** Write new subscriptions as
   `#[subscribe]` streams, following
   `features/task/task/task/src/{service.rs,backend.rs}`. Don't
   convert an existing one as a drive-by — it changes method ids (see
   the schema-skew gotcha below).

3. **Cross-slice references go through ids, not value types.** A
   `TaskInfo.project_id: Option<Uuid>` is fine; a `TaskInfo` holding a
   `ProjectInfo` is not. Each proto stays an acyclic leaf.

4. **Server-only crates must not leak into the wasm graph.** Gate
   native-only code with `#[cfg(not(target_arch = "wasm32"))]` at the
   `lib.rs` level.

### UI

1. **architect-ui primitives only.** Don't hand-roll Buttons / Cards /
   Sheets / Dialogs / Comboboxes / Sidebars. architect-ui is **in-tree** at
   `libs/architect-ui/architect-ui` (a path dep) — if a primitive is missing a
   feature, fix it there; the edit propagates on the next
   `cargo check`. Prelude:
   `libs/architect-ui/architect-ui/src/prelude.rs`.

2. **Theme tokens, never hex.** `bg-background` / `text-foreground` /
   `bg-card` / `border-border` / `bg-primary` / `text-muted-foreground`
   / `bg-muted` / `bg-accent`. Never `bg-slate-900` or `#1a1a1a`. The
   CSS variables flip per light/dark and per org/project theme
   override; a hardcoded color breaks every preset.

3. **Dark mode is the default.** Both modes must look correct with no
   `dark:` overrides. A different palette per mode is a theme-token
   concern, not a component concern.

4. **Two-tier theming.** Organization picks a preset; a project can
   override. `architect_ui::ThemeProvider` wraps the App root; `ThemeScope`
   wraps a project route when an override is active. Read from and
   write through the contexts in `crates/task/ui/src/theming.rs` —
   don't bypass them.

5. **Dumb components.** Feature `*-ui` crates own no state: data in
   via props, events out via `EventHandler<T>`. Signals and clients
   live in the page (`crates/task/ui/src/pages/`). This keeps
   components portable across web/desktop/mobile and usable in
   ui-lab.

6. **No `.to_string()` on Dioxus prop literals.** Props taking
   `Into<String>` accept `&str` directly in Dioxus 0.7+.
   `class: "foo bar"`, not `class: "foo bar".to_string()`. Same for
   ternary arms.

### Engineering

1. **`cargo check` before claiming done** — native *and* wasm. See
   "Verify before done" below.
2. **One cargo command at a time per worktree.** The target-dir lock
   is shared; concurrent builds clobber fingerprints. Always set an
   explicit timeout on build/test invocations.
3. **Don't fabricate APIs.** Read the source: architect-ui's prelude,
   `libs/architect/macros/architect-derive/` for the Entity macro,
   `libs/architect/crdt/crdt/src/codec.rs` for codec helpers.

---

## Verify before done

```bash
# native — the crates you touched
cargo check -p task-server
cargo check -p ui                 # the app shell (NOT -p task-ui)
cargo check -p task-cli

# wasm — the browser build
cargo check -p task-app-web --target wasm32-unknown-unknown

# tests for the slice you touched
cargo test -p <crate>
```

Warnings are acceptable; errors are not. Full gates: `just ci` from
`apps/task/` (fmt + clippy `-D warnings` + nextest).

**Never run cargo while a `dx serve` is running in the same worktree**
— they fight over the target lock.

---

## Common gotchas

### Proto changes require a task-server rebuild (schema skew)

Changing any proto changes vox method ids (they hash the method's name
+ payload shapes), so a **running `task-server` built before the change
can't talk to freshly built clients**. The failure mode is opaque:
`structural mismatch` / `InvalidPayload` / `Unknown method`.

**Rule: after touching a proto, rebuild + restart task-server before
trusting any live behavior.**

The guard: the server publishes per-service schema stamps at
`/.well-known/task-server.json` (`task_server::schema_stamps`,
`org_proto::schema_stamp`). `task doctor` compares your CLI build
against the running server and exits non-zero on mismatch; ui-lab's
`pnpm smoke` does the same for the generated TS bundle and downgrades
skew-shaped failures to loud `SKEW SKIP` warnings.

### Mixed-target cargo check false alarms

`error: This wasm target is unsupported by mio` while running
`cargo check -p task-server -p task-app-web --target
wasm32-unknown-unknown` is **not a real error** — it's mio (a
native-only dep of task-server) being asked to compile for wasm. Check
the two separately.

### dx serve

- Serve the web build with `--hot-patch false`. The default
  hot-patching path produces `LinkError` / subsecond panics on edit.
- Hot-patch does not pick up new RSX *attributes* (`id=`,
  `data-testid=`), only function-body changes. If a selector test
  can't find an element you just added, use `just test-browser-fresh`.

### architect-ui

- `StatusBadgeVariant` is **`Success / Warning / Danger / Neutral`
  only**. No `Error`, no `Info`.
- `ButtonVariant` has **no `Default`** — use `Primary`.
- `ButtonSize` has **no `Default`** — use `Medium`.
- `PopoverContent` / `DropdownContent` take a `side` prop
  (`top`/`right`/`bottom`/`left`). Near the bottom of a scroll
  container (sidebar footer), use `side="top"` or the menu falls below
  the viewport.
- `Slider`'s value is `Signal<f64>` only — no `on_change`. Use the
  signal.
- `Textarea` has **no `on_keydown`**. Workarounds: a sibling
  `tabindex=-1` capture div, or upstream a prop.
- `Combobox` / `ComboboxTrigger` don't take rich children; the trigger
  renders the raw value. Render badges/icons as siblings.
- `Checkbox` and `Switch` need `Signal<bool>`, not `bool`. Inside a
  `for` loop, materialize with `use_signal` per row.

### Lucide icons (via `architect_ui::lucide_dioxus::*`)

- `CircleCheck` — not `CheckCircle2`
- `TriangleAlert` — not `AlertTriangle`
- `House` — not `Home`
- `Ellipsis` — not `MoreHorizontal`
- `CircleStop` — the stop button
- `CornerDownLeft` / `SendHorizontal` — the send button
- Icon size is `usize`, not `u32`/`i32`.

### Dioxus

- `use_signal(|| init)` for state. Inside `use_effect`, `.read()`
  subscribes — use `.peek()` to read without subscribing (critical
  when avoiding update loops between bridging effects).
- `spawn(...)` ties the task to the current component's scope. A task
  spawned inside a sheet/dropdown/modal that closes on selection is
  **silently cancelled mid-flight**. For actions triggered from
  self-closing UI, run a root-owned `use_coroutine` service and
  `send()` it a message from every surface — see
  `crates/task/ui/src/auth.rs`. Sequential consumption also kills
  concurrent-action races for free.
- `Event<FormData>::value()` on contenteditable returns the element's
  `textContent`. Anything rendered inside becomes part of `value()`.
  Keep non-editable chrome (list bullets, heading hash, blockquote
  `>`) OUTSIDE the contenteditable as a flex sibling, or every
  keystroke duplicates the prefix.
- `use_memo(use_reactive!(...))` for derived values that must re-run
  when inputs change.
- `wasm-split` is experimental and needs `lto = true` + `debug = true`;
  disabled by default.

### Build / dev shell

- The repo-root `.envrc` is `use flake`; direnv loads the dev shell on
  `cd`. The root flake exposes `default`, `ci`, and `reaper-test` —
  **there is no `.#ui` or `.#playwright` shell**. The default shell
  carries the toolchain, `dx`, tailwindcss, and
  `PLAYWRIGHT_BROWSERS_PATH`.
- `cargo check --workspace` from `apps/task/` checks the entire
  ~160-member monorepo, not just Task. Use `-p` when you mean Task.

### rust-analyzer staleness

rust-analyzer often reports "file not found" / "missing field"
diagnostics that don't reflect actual compile state. **Trust `cargo
check`, not the LSP.** Restart the server after large file moves.

---

## Tracking

This project does NOT use bd/beads or any external ticket tracker. Do
not add a `bd hooks run` call to the git hooks. What we rely on:

- **`plans/<topic>.md`** — architectural follow-ups. Every plan
  carries a status header; see `plans/README.md` for the vocabulary
  and for where finished plans go (`plans/done/`). Write the plan
  before the second commit of any multi-slice arc.
- **`task issue` / `task code`** — the in-repo issue + git workflow
  (see the `task` skill and `docs/task-code-workflow.md`).
- **Commit messages** — the activity log. Descriptive subject,
  phase-grouped body, verify status.
- **`// FUTURE:` comments** for narrow follow-ups too small to be a
  plan. Visible via `rg -t rust 'FUTURE:'`.
- **README.md / AGENTS.md** — slowly-changing canonical guidance.
  Update when conventions shift. If you find a claim in here that the
  code contradicts, **fix the doc in the same PR** — that's how this
  file rotted the first time.

Don't introduce new tracking systems (markdown TODO files,
scratchpads, GitHub Issues without explicit user request).

---

## Landing the plane (session completion)

1. **Write up loose ends** — architectural follow-ups to
   `plans/<topic>.md` (with a status header); narrow gaps to a
   `// FUTURE:` comment at the call site.
2. **Run quality gates** if code changed — see "Verify before done".
3. **Update issue status** — close finished work.
4. **Commit** — descriptive message; don't commit code that doesn't
   compile.
5. **Push only if the user asked.** `git pull --rebase && git push`,
   then confirm `git status` shows up to date.
6. **Hand off** — a short summary of state for the next session; if
   the arc is mid-flight, a note in `plans/handoff/`.

---

## Non-interactive shell commands

Always use non-interactive flags — `cp`/`mv`/`rm` may be aliased to
`-i`:

```bash
cp -f source dest      rm -f file       rm -rf directory
mv -f source dest      cp -rf source dest
```

Others that may prompt: `scp`/`ssh` (`-o BatchMode=yes`), `apt-get`
(`-y`), `brew` (`HOMEBREW_NO_AUTO_UPDATE=1`).

---

## Where things live

| Need | Path |
|---|---|
| App shell + router | `crates/task/ui/src/app.rs`, `.../routes.rs` |
| Pages (where state + clients live) | `crates/task/ui/src/pages/` |
| Theming contexts | `crates/task/ui/src/theming.rs` |
| vox session / URL resolution | `crates/task/ui/src/vox_session.rs` |
| Server entry + org router | `apps/task/server/src/main.rs`, `.../lib.rs` (`router`, `org_layer_router`) |
| CLI | `apps/task/cli/src/main.rs` |
| A slice's wire contract | `features/task/<slice>/<slice>-proto/src/` — or `<slice>/src/service.rs` when there's no proto crate |
| architect-ui prelude | `libs/architect-ui/architect-ui/src/prelude.rs` |
| architect Entity macro | `libs/architect/macros/architect-derive/` |
| architect crdt codec helpers | `libs/architect/crdt/crdt/src/codec.rs` |
| Env var reference | `.env.example` (complete inventory) |
| Vault ⇄ CRDT design | `docs/architecture/vault-crdt-reconciliation.md` |
| Dev demo seed (studio dataset, deterministic names) | `docs/dev-seed.md` |
| Crate topology + request path | `ARCHITECTURE.md` |
| Plans + their status convention | `plans/README.md` |

## Known stale (do not trust, fix if you touch)

- **`crates/task/xtask`** — `cargo xtask build` targets
  `integrations/obsidian/plugin`, deleted; `cargo xtask codegen`
  resolves its out-dir to `crates/task/ui-lab/` instead of
  `apps/task/ui-lab/`.
- **`apps/task/nix/module.nix`** — an orphan NixOS module (nothing in
  the flake imports it) that sets `TASK_VAULT`, `TASK_DB_PATH`,
  `TASK_SEED_DEMO`, and `NEXTCLOUD_*`. The server reads none of them.
  The live deploy path is `apps/task/deploy/chart/`.
- **`docs/self-host.md`** — describes the same dead env vars and CLI
  commands (`task sync`, `task people`, `task invoice`, `task server`)
  that no longer exist.

## When in doubt

1. **Search, don't assume.** `rg -t rust "thing"` beats a guess, and
   beats this document if the two disagree — the code is the truth.
2. Read the nearest slice before copying it; the shapes vary.
3. Trust `cargo check`, not rust-analyzer. Native and wasm are
   separate targets; check both.
4. When stuck, write the plan first in `plans/` and ask the user to
   review before launching agents.
