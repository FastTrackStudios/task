# Task webapp baseline

The first Task webapp slice lives in `apps/task/web` (package `task-app-web`) and reuses the shared Dioxus UI crate at `crates/task/ui` (package `ui`). It is intentionally a shell/status/navigation baseline: the data is in-memory demo data and does not read or mutate production Task state.

## Stable package build

Build the deployable static bundle from the repo root:

```bash
nix --extra-experimental-features 'nix-command flakes' build --no-write-lock-file .#task-webapp
```

The package writes static files under:

```text
result/www/
```

The Nix build runs the Tailwind pipeline before the Dioxus build:

```bash
tailwindcss -i apps/tailwind.css -o apps/web/assets/tailwind.css
cd apps/web
dx build --release --platform web --debug-symbols false --wasm-split --features wasm-split
```

(`just web-release` runs the same two steps locally.)

## Bundle size

Measured on the same tree (uncompressed wasm / brotli -q 9, which is
what the static server sends). "First load" is what a fresh visit to
`/` downloads before it renders: the main chunk plus the landing
route's chunk.

| build | first load wasm | brotli |
|---|---|---|
| dx's ad-hoc `wasm-release` (inherits release, `opt-level = "s"`) — what production shipped | 71.0 MB | 19.07 MB |
| `[profile.wasm-release]` below (`z`, fat LTO, 1 CGU, `panic = "abort"`) | 54.6 MB | 16.72 MB |
| + one engraver (the `[patch]` in the root `Cargo.toml`) | 53.5 MB | 16.67 MB |
| + `--wasm-split` (main chunk 18.3 MB + Home route 0.15 MB) | 18.5 MB | 5.4 MB |
| + lazy player, engraver, widgets, panels and store providers (main 6.8 MB + Home route 0.15 MB + 7 provider chunks 0.26 MB) | **7.2 MB** | **1.93 MB** |
| same profile with `opt-level = "s"` instead of `"z"`, unsplit, for the record | 58.7 MB | 17.69 MB |

The split bundle is 54 lazy chunks next to the main one. The main chunk
is the shell — router, nav, auth, the vault explorer, the home and task
pages' shared code — and nothing plugin-shaped: everything an app
contributes beyond its nav entries and link claims sits behind a
boundary of its own. Where each subsystem lives, and when it downloads:

| subsystem | chunk | raw / brotli | downloaded when |
|---|---|---|---|
| shell (router, nav, auth, explorer, stores machinery, fuzzy search) | `task-app-web_bg` (main) | 6.8 MB / 1.79 MB | first load |
| each route page | `module_*_route<Name>Route*` | 0.003–2.5 MB | the route is visited |
| the vault/editor route (markdown editor, tree-sitter, typst, mermaid, the chart pane's engraver copy) | `module_*_routeVaultRoute*` | 31.1 MB / 10.9 MB | a note is opened |
| song / setlist note widgets (the multitrack player, daw worklet bytes, `daw-standalone` + `symphonia`, the in-tab session engine) | `module_*_player_note_widget` | 11.2 MB / 3.44 MB | a `type: song` / `type: setlist` note is opened |
| chart fences (`editor-keyflow`: engraver + notation fonts) | `module_*_engrave_fence` | 4.4 MB / 1.86 MB | a ```` ```kf ```` fence is first rendered |
| the global now-playing engine + setlist-row highlighter | `module_*_player_engine` | 0.49 MB / 0.18 MB | the first play request |
| the agent dock panel | `module_*_agent_panel` | 0.74 MB / 0.25 MB | the dock is opened |
| the note inspector's local graph (the knowledge-graph layout + SVG renderer, shared with `/graph`) | `module_*_local_graph` | not yet measured — see below | the inspector's Graph tab is first opened, on a vault note or a wiki page |
| each plugin's screens | `module_*_<app>_screen` | 0.15–0.82 MB | the app is visited |
| each plugin's store providers (7 apps) | `module_*_provide_stores` / `provide_all` | 3–144 KB each | at boot, after the shell paints |
| `type: video` note widget | `module_*_video_note_widget` | 0.15 MB | a video note is opened |

The local-graph chunk has no number yet: at the time it was added the
split build (`just web-release`) died in dx's splitter (a walrus
`!self.dead.contains(&id)` assertion while emitting the main module) on
`main` itself, before and after the change, so nothing could be
measured. It is behind `task_plugin_ui::lazy_element_with!` — the
`lazy_element!` shape with an argument — reached only from the vault and
wiki route chunks, so the main chunk cannot have grown; fill the sizes
in from the next split build that completes.

Before this, the main chunk carried the player, the engraver and every
plugin's providers and panel — 18.3 MB — because the shell mounted them
outside the router `Outlet` (the now-playing engine), or registered them
at boot (widget renders, chart fences, `provide`), so no route boundary
covered them. The boundaries that moved them are the SDK's
`task_plugin_ui::lazy_element!` (a mounted surface), `lazy_render!` (a
widget's block view; its *matches* stay in main), `lazy_provide!` (an
app's store providers, installed at the root once their chunk arrives —
`use_deferred_providers` in `App` runs them last, and every lazy
surface waits on that before rendering, so no screen can ask for a
store that is not there yet) and, for the chart fences, a
`FenceRenderer` that declines while its chunk downloads and re-runs the
editor's decoration pass when it lands. The `wasm-split` cargo feature
gates all of it; desktop, mobile and `dx serve` call straight through.

The splitter does not pool code that two split points share (only code
shared with *main* stays in main), so two boundaries reaching the same
big thing each carry a copy — which is why the song and setlist widgets
are one boundary, and why the vault route still carries an engraver
alongside the fence chunk's. Chunks are content-hashed, so they cache
for as long as the static server lets them.

What remains in main and why: the fuzzy matcher (`neo_frizbee`, ~1 MB,
the command palette and search), the shell's own pages' shared code,
`files-ui` (the explorer and the review player host, which the shell
mounts), and the store/atom machinery every page uses. The 7 provider
chunks (0.26 MB in all) still download at boot, because a store has to
exist above the router before any page can read it; making them wait
for their app's first screen would drop the live subscriptions the
`stream:` stores exist for.

The release bundle is governed by:

- **`[profile.wasm-release]`** in the root `Cargo.toml` — dx's default
  profile name for a web release build, defined explicitly so
  `opt-level = "z"`, fat LTO, one codegen unit and `panic = "abort"`
  apply. The server's `release` profile is untouched.
- **`[web.wasm_opt]`** in `apps/web/Dioxus.toml` — dx runs binaryen's
  `wasm-opt -Oz --strip-debug` on every chunk (its default level is
  already `z`, so nothing is set).
- **One copy of each git crate.** The `[patch]` tables at the bottom of
  the root `Cargo.toml` collapse the keyflow repo's and FastTrackStudio's
  copies of `engraver`/`engraver-proto`/`keyflow-text` into one. Check
  with `cargo tree --manifest-path apps/web/Cargo.toml --target
  wasm32-unknown-unknown -i engraver-proto` — one root, not an
  "ambiguous specification" error.
- **`--wasm-split` + `--features wasm-split`.** The cargo feature on
  `task-app-web` turns on `dioxus/wasm-split` +
  `dioxus-router/wasm-split` (every `#[route]` component becomes a
  lazily fetched chunk via the router macro's `maybe_wasm_split!`) and
  `task-plugin-ui/wasm-split` (`task_plugin_ui::lazy_view!` puts each
  plugin app's screens in a chunk of their own — scripture, notation,
  email, finance, …). The dx flag runs the splitter that writes those
  chunks (`assets/module_*_<name>.wasm`, shared code in
  `assets/chunk_*.wasm`, loader glue in `assets/__wasm_split.js`). The
  feature and the flag must travel together: the feature compiles
  loaders that import from the file the flag produces.

### The dx it needs

The published `dioxus-cli 0.8.0-alpha.0` panics splitting this app,
after emitting all 41 chunks, while emitting the main module:

```text
walrus-0.23.3/src/module/functions/mod.rs:186: assertion failed: !self.dead.contains(&id)
```

That is [dioxus#4769](https://github.com/DioxusLabs/dioxus/issues/4769).
The cause is in `wasm-split-cli` (diagnosed in
[dioxus#5668](https://github.com/DioxusLabs/dioxus/pull/5668)): it
matched the pre-bindgen module's mangled names against wasm-bindgen's
demangled ones, so almost no function matched, the fallback that should
have kept unmatched symbols in the main module wrote to a map nobody
read, and the walrus gc pass then walked ids already pruned. Dropping
`codegen-units = 1` — the workaround suggested on the issue — does not
help here (measured). The dev shell's and the hermetic build's `dx`
therefore come from that PR's branch (`nix/modules/dx.nix` builds
`packages/cli` from `Brahmastra-Labs/dioxus@19ea842`, upstream
`main@e1c6342` + two commits). The PR's second commit fixes two
`dioxus-core` suspense bugs that corrupt renders when nested boundaries
resolve out of order — and a split app is nothing but nested boundaries:
clicking through five routes in a second on a build without it gave
`cannot reclaim ElementId(N)` and a blank screen whose chunk was never
requested. So `dioxus-core` (with its in-repo deps, so every type has
one copy) is `[patch]`ed onto the same fork rev in the root
`Cargo.toml`; the rest of the workspace stays on upstream `f717a8e`,
which is that PR's base plus the Blitz beta.1 sync (`packages/native*`
only). Move dx and the patch back to upstream once the PR is in a
release.

`dx serve` (and `just live`) build without the feature or the flag, so
the dev loop is one plain module and nothing about splitting touches it.

## Local preview/dev mode

Use the Dioxus flake dev shell and keep preview data isolated from stable Task services:

```bash
nix --extra-experimental-features 'nix-command flakes' develop .#ui -c sh -lc '
  tailwindcss -i apps/task/web/tailwind.css -o apps/task/web/assets/tailwind.css
  cd apps/task/web
  dx serve --platform web
'
```

## FTS-ui integration status

FTS-ui was tested as the intended design-system dependency, but it is blocked for the WASM web target in this slice. The exact failure is in `fts-story-core`, pulled by `architect-ui` through always-on story registration:

```text
error: distributed_slice is not implemented for this platform
/home/cody/Development/FastTrackStudio/fts-story/crates/fts-story-core/src/lib.rs:33:1
```

That comes from `linkme::distributed_slice` while compiling `fts-story-core` for `wasm32-unknown-unknown`. Until `architect-ui` gates story registration behind a non-web feature or publishes a web-safe package/API, the Task webapp uses local Tailwind/Dioxus components that match the FTS visual direction without depending on the live `../FastTrackStudio` checkout.
