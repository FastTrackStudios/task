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
dx build --release --platform web --debug-symbols false
```

(`just web-release` runs the same two steps locally.)

## Bundle size

Measured on the same tree (uncompressed wasm / brotli -q 9, which is
what the static server sends):

| build | wasm | brotli |
|---|---|---|
| dx's ad-hoc `wasm-release` (inherits release, `opt-level = "s"`) — what production shipped | 71.0 MB | 19.07 MB |
| `[profile.wasm-release]` below (`z`, fat LTO, 1 CGU, `panic = "abort"`) | 54.6 MB | 16.72 MB |
| + one engraver (the `[patch]` in the root `Cargo.toml`) | 53.5 MB | 16.67 MB |
| same profile with `opt-level = "s"` instead of `"z"`, for the record | 58.7 MB | 17.69 MB |

The release bundle is governed by three things, all in this repo:

- **`[profile.wasm-release]`** in the root `Cargo.toml` — dx's default
  profile name for a web release build, defined explicitly so
  `opt-level = "z"`, fat LTO, one codegen unit and `panic = "abort"`
  apply. The server's `release` profile is untouched.
- **`[web.wasm_opt]`** in `apps/web/Dioxus.toml` — dx runs binaryen's
  `wasm-opt -Oz --strip-debug` on the output (its default level is
  already `z`, so nothing is set).
- **One copy of each git crate.** The `[patch]` tables at the bottom of
  the root `Cargo.toml` collapse the keyflow repo's and FastTrackStudio's
  copies of `engraver`/`engraver-proto`/`keyflow-text` into one. Check
  with `cargo tree --manifest-path apps/web/Cargo.toml --target
  wasm32-unknown-unknown -i engraver-proto` — one root, not an
  "ambiguous specification" error.

### Bundle splitting — prepared, blocked on dx

The code side of lazy loading is in place and off by default:

- `--features wasm-split` on `task-app-web` turns on
  `dioxus/wasm-split` + `dioxus-router/wasm-split` (every `#[route]`
  component becomes a lazily fetched chunk via the router macro's
  `maybe_wasm_split!`) and `task-plugin-ui/wasm-split`
  (`task_plugin_ui::lazy_view!` puts each plugin app's screens in a
  chunk of their own — scripture, notation, email, finance, …).
- `dx build --wasm-split` runs the splitter that writes those chunks
  (`assets/module_*_<name>.wasm`, shared code in `assets/chunk_*.wasm`,
  loader glue in `assets/__wasm_split.js`). The cargo feature and the
  dx flag must travel together: the feature compiles loaders that
  import from the file the flag produces.

With the pinned dx (`dioxus 0.8.0-alpha.0`, nixpkgs-dx) the splitter
panics on this app after emitting all 41 chunks, while emitting the
main module:

```text
walrus-0.23.3/src/module/functions/mod.rs:186: assertion failed: !self.dead.contains(&id)
```

That is `wasm-split-cli`'s `create_ifunc_table` looking up a function
its `prune_main_symbols` step already deleted — a function reachable
only from split modules but also needed in the shared indirect-function
table. It reproduces with route splitting alone (plugin boundaries off)
and with dx's own preferred split profile (`opt-level = "s"`,
`debug = true`), so it is not a profile interaction. Once dx is bumped
past it, add `--wasm-split --features wasm-split` to the `dx build` line
in `nix/modules/packages/web-bundles.nix` and to `just web-release`;
`cargo check -p task-app-web --target wasm32-unknown-unknown --features
wasm-split` already passes, and the brotli pre-compression in the nix
package already covers every `.wasm`/`.js` under `www/`.

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
