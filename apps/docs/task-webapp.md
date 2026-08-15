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
tailwindcss -i apps/task/web/tailwind.css -o apps/task/web/assets/tailwind.css
cd apps/task/web
dx build --release --platform web
```

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
