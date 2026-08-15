# Plugin system — core Task stays, everything else becomes a plugin

Status: runtime toggle LIVE (2026-07-28). The contract crate, manifest
field, CLI verbs, server assembly, API reporting, and the UI gates are
wired; what remains is build-time exclusion (cargo features), the
settings panel, and the mechanical rollout of the route gate to the
remaining plugin routes (see "What's done / what remains" below).

## The idea

The core Task model (tasks, projects, the vault, orgs, auth) is the
platform. Everything domain-flavoured — meal planning, fitness,
FastTrackStudio's setlist/song surfaces, email, forges, scripture — is a
**plugin**: a named bundle of feature slices that an org can turn on and
off, and that a build can compile out entirely.

Two independent switches, deliberately:

| Switch | Mechanism | What "off" means |
|---|---|---|
| Build-time | cargo features on the assembling apps | the plugin's crates are not compiled; smaller binaries, faster builds |
| Runtime, per org | `plugins` in the org's `org.toml` (`OrgManifest`) | services not mounted (wire calls fail with not-found), nav hidden, routes render a "plugin disabled" notice, widgets unregistered |

Runtime toggling is the product feature; build-time exclusion is the
engineering feature. Neither uses dynamic loading — `dlopen` is a
non-starter for the wasm target and a liability everywhere else. A
"plugin" is a statically linked crate that *registers* contributions;
the registry decides what is active.

## The contract

A plugin is identity + contributions. Identity lives in one wasm-clean
crate; contributions are per-surface, because the server and the UI are
different binaries with different targets:

- **`task-plugin`** (new, `crates/task/plugin`) — the shared vocabulary:
  `PluginId`, `PluginInfo { id, name, description, core }`, the catalog
  of known plugins, and `PluginSet` — the resolution from an org's
  manifest to the enabled set (core plugins are always on; unknown ids
  are warned about and ignored, so an org.toml written by a newer build
  still loads on an older one).
- **Server contributions** — each plugin exposes a
  `fn server_plugin() -> ServerPlugin` carrying its service mounts
  (descriptor + serve + permit table + stream layers). The org router
  assembles from the enabled set. `permits::mounts()` becomes the
  concatenation of plugin contributions — the existing
  `permits_cover_router` guard keeps holding, per plugin.
- **UI contributions** — each plugin's `-ui` crate exposes a
  `fn ui_plugin() -> UiPlugin` carrying nav entries, widget specs (the
  `task-widgets` registry), and store registrations. The shell's `Route`
  enum stays static (Dioxus's router is an enum — routes cannot be
  dynamic), so a disabled plugin's routes stay routable but render a
  standard "this plugin is off for this org" panel, and its nav entries
  disappear. Compile-time exclusion removes the routes for real.
- **CLI contributions** — clap's derive is also static; the CLI keeps
  its command enum but consults the org's `PluginSet` before running a
  plugin command, failing with "the <x> plugin is disabled for this org
  (enable with `task org plugins enable <x>`)".

Registration is **explicit**: the app roots call
`registry.register(mealplan::plugin())`. No linker collection, no
`inventory` — explicit is debuggable, wasm-safe, and makes the
build-time feature gates one-line (`#[cfg(feature = "plugin-mealplan")]`
around the registration).

## Proposed plugin grouping

Core (always on, not toggleable): task, project, goal, milestone,
workstream, inbox, vault, view, tag, label, links, prefs, org, identity,
share, attachments, media, timer.

| Plugin id | Slices | Notes |
|---|---|---|
| `mealplan` | mealplan, pantry, cookbook, shopping, substitutions, recipe-import | the worked example |
| `fitness` | body, exercises, workouts, intake | |
| `fasttrackstudio` | song embeds, setlist/session player (`task-player-ui`), keyflow chart surfaces | the FTS product tie-in |
| `wiki` | wiki-* | big enough to be its own toggle |
| `scripture` | scripture, scripture-ui | |
| `email` | email-* | already effectively optional |
| `forge` | git-*, issue/review surfaces | |
| `agent` | agent-* | |
| `scheduling` | scheduling, calendar/booking surfaces | |
| `finance` | finance, finance-db, invoicing | timer stays core; billing is the plugin |
| `contacts` | contacts | |
| `recall` | recall | |
| `home` | locations, inventory | physical-world ops |

Grouping is a product call — this table is the proposal, trivially
adjustable while everything registers through one catalog. As
implemented, the authoritative per-service assignment is the `plugin`
field on `permits::Mount` (apps/task/server/src/permits.rs). Calls made
for services the table didn't name: `threads`, `timer`, `inbox`,
`tags`, `links`, `resources` (the generic resource-library reader:
bible editions, transcripts, song media all live under it) are core;
`collection` (Library/Setlist/Show/Playlist) is `fasttrackstudio`.

## What's done / what remains

Done (this branch):

- `permits::Mount.plugin` + `mounts_for(set)`; `mounts()` stays the
  full build catalog and `schema_stamps()` stays complete (skew
  detection is build-level).
- `org_layer_router` consults `OrgAppState::plugins` (resolved once
  from `OrgManifest.disabled_plugins` in `build_org_state`); a disabled
  plugin's services are not mounted (wire = unknown service) and the
  permission gate installs permits only for mounted services
  (`permits::install_for`). `permits_cover_router` proves both the
  plain and deny-list views; `plugin_toggle_e2e` covers the wire + API
  behaviour.
- `/org/{slug}/api`: per-service `"plugin"` + `"mounted"` flags plus a
  top-level `"plugins"` catalog with per-org enabled state (disabled
  services are listed-with-flag, not omitted). `task api` shows the
  plugin column and `[DISABLED]` marks from the local active org.
- Well-known doc carries `disabled_plugins` per org; the shell resolves
  the ACTIVE org's set (`task_ui_core::orgs::active_plugin_set`) and
  gates nav (`nav_tabs_for`), widgets
  (`WidgetRegistry::set_plugin_set`), and routes (`PluginGate` →
  `PluginDisabledPanel`).
- `task org plugins list|enable|disable`.

Route-gate rollout: DONE — every non-core route shim in
`crates/task/ui/src/routes.rs` wraps its page in `PluginGate`:
mealplan (plan / cook / edit-recipe), fitness, recall, contacts,
email, scripture, wiki (+ page / sources / source routes), agents,
repos + connections (forge), schedule + bookings (scheduling),
finances + invoices + ledger (finance), locations + inventory (home),
and watch (fasttrackstudio — the setlist/song session surfaces are
vault-note embeds and widgets, already gated via the widget registry;
the Watch nav tab retagged from core to fasttrackstudio to match).

Cargo features per plugin: DONE for `task-server` (complete) and
`task-cli` (partial — see the entanglement notes); NOT attempted for
`crates/task/ui` (blocked, see below).

- **task-server** — thirteen `plugin-*` features, default = all (a
  plain build is behaviour-identical to the pre-feature tree). A
  feature gates: the plugin's dependency crates (`optional = true`),
  its `OrgAppState` fields + backend construction in `build_org_state`,
  its mount group in `org_layer_router`, and its permit tables + the
  matching `permits::mounts()` entries — so the catalog, the stamps,
  and the router shrink together and `permits_cover_router` keeps
  proving they agree under ANY feature set. Cross-cuts that needed a
  finer knife: `link_sync` (core) only mints `note → verse` edges under
  `plugin-scripture` (note→note wikilink sync stays core); the MCP
  calendar tools (`list_events` / `create_event` / `reschedule_event` /
  `cancel_event` + `task_context`'s event count) ride
  `plugin-scheduling`; the forge webhook route, the forge-sync poll
  loop, and the `ForgeSyncTaskService` decorator on TaskService ride
  `plugin-forge` (without it the raw `TaskBackend` serves directly).
  Plugin-owned integration tests carry `#![cfg(feature = …)]`.
- **task-cli** — the same thirteen features, each forwarding to the
  matching `task-server/plugin-*` (the `TASK_EMBED=1` in-process server
  loses exactly what a remote build would). A compiled-out plugin's
  command still parses (hidden `NotCompiled` trailing-args variant) and
  fails with "the `<x>` plugin is not compiled into this build". Fully
  excluded crates: wiki (wiki-live/-graph/-search/-archive/-proto,
  agent-wiki), fasttrackstudio (collection-proto, song), finance
  (finance, finance-proto, finance-db), fitness (body, exercises,
  workouts, intake), mealplan's recipe-import + cooklang, and
  locations/pantry/cookbook (each compiled for whichever of home /
  mealplan / fitness is on). **Deps pinned by CORE commands, gated at
  the command surface only**: `mealplan` + `scheduling-proto` (`task
  brief` reads meals, day plans and bookings; `plan.rs` helpers back
  `brief`), `agent-proto`/`agent-codex`/`agent-inbox` (`task inbox`
  talks to agent backends; `task issue` renders agent prompts), and
  `git-proto`/`git-config`/`git-forgejo`/`git-github` (`task issue` /
  `code` / `setup` are forge-native workflow commands, deliberately
  core).
- **crates/task/ui** — deliberately NOT feature-gated. The blocker is
  structural, not a dependency cycle: `stores.rs` + `feeds.rs` register
  every slice's stores/feeds in single blocks, the pages cross-reference
  plugin protos from core surfaces (`project_detail` → finance + forge +
  agent; `invoices` → contacts; `schedule` → mealplan; `note_view` →
  scripture), and the `Route` enum + pages live in one crate. Build-time
  UI exclusion needs the pages and store registrations to move into
  per-plugin `-ui` crates first; until then the runtime gates (nav,
  widgets, `PluginGate`) are the UI story. Revisit after a pages split.

CI check commands (all must stay green; add to the checks workflow):

```bash
# default = all plugins — byte-identical pre-feature behaviour
cargo test -p task-server --test permits_cover_router
# core-only — catalog, stamps, router and permit tables shrink together
cargo test -p task-server --no-default-features --test permits_cover_router
cargo check -p task-server --no-default-features
cargo check -p task-cli --no-default-features
```

Other remaining work:
- **Settings panel** — the org-admin UI over the same manifest field
  the CLI writes (needs a server RPC to edit `org.toml` remotely; the
  CLI path is local-only today).
- Per-slice backend construction skip in `build_org_state` (pure
  optimization — deliberately not done: every backend is a cheap
  vault/path handle or a pool another slice shares, and the fields are
  non-optional; not mounting is the requirement).
- The MCP tool surface (`/org/{slug}/mcp`) does not yet consult the
  plugin set — its tools are core-leaning, but a sweep is due when the
  settings panel lands.

## Sequencing

1. ~~**Now**: `task-plugin` crate (ids, catalog, `PluginSet`), and
   `OrgManifest.plugins` — additive, default "all enabled", so nothing
   changes behaviour until assembly wires up.~~ DONE.
2. ~~**After the realtime + widgets + apidocs branches merge**: server
   assembly (mounts from plugins), UI assembly (nav/widgets/stores from
   plugins), `task org plugins list|enable|disable`, and
   `/org/{slug}/api` gaining a `plugin` field per service.~~ DONE
   except the settings panel (see above).
3. ~~**Then**: cargo features per plugin on `task-server` and the
   CLI.~~ DONE (server complete, CLI partial — see above); the CI
   matrix job (minimal/full) still needs wiring into the checks
   workflow, and the ui-crate gating waits on a pages split.

## Non-goals

- Dynamic loading of third-party code. External crates join by being
  added to the workspace and registered — the contract makes that a
  small, documented step, not a runtime capability.
- Per-user toggles (v1 is per-org; the manifest is org state).
- Data migration on disable. A disabled plugin's vault files and tables
  stay; the plugin just stops being served. Re-enabling picks them up
  again. Deleting data is a separate, explicit operation.
