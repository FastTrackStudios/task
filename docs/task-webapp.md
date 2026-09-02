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
| + `--wasm-split` (main chunk 18.3 MB + Home route 0.15 MB) | **18.5 MB** | **5.4 MB** |
| same profile with `opt-level = "s"` instead of `"z"`, unsplit, for the record | 58.7 MB | 17.69 MB |

The split bundle is 41 lazy chunks next to the main one. The ones that
matter: the vault/editor route (`module_*_routeVaultRoute*`, 30.6 MB /
10.7 MB br — the markdown editor, tree-sitter grammars, the notation
engraver and its fonts, the session player) downloads the first time a
note is opened; the schedule route is 2.5 MB; every other route and
plugin screen is 0.1–0.7 MB. Chunks are content-hashed, so they cache
for as long as the static server lets them.

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
