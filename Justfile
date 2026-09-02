# Task recipes.
#
# This file lives at the repo root — `just` sets the working directory
# to this file's directory, so every relative path below is relative to
# the repo root. There is ONE cargo workspace, rooted here, so
# `--workspace` means the whole repo; use `-p <crate>` to narrow.
#
# Run commands: just <recipe-name>

# Default: check the workspace
default: check

# ── CLI ──────────────────────────────────────────────────────────────────

# Run the task CLI
task *args:
    cargo run -p task-cli -- {{args}}

# ── Run the app ──────────────────────────────────────────────────────────
#
# Two recipes for two terminals (or `just dev` to run them both):
#   1. `just server` → task-server on :9090
#   2. `just web`    → Dioxus dev server on :8765
#
# There is no separate migrate/seed step. `task-server` resolves its
# data root from `$TASK_DATA_ROOT` (default `$HOME/.task`), creates
# `orgs/<slug>/` on demand, and runs each service's sea-orm migrations
# at boot. Point `TASK_DATA_ROOT` at a throwaway dir for a clean slate.

# Dioxus dev server for `web/` (package `task-app-web`) on port 8765.
# Binds 0.0.0.0 so the starcommand nginx reverse proxy reaches it via
# the 10G LAN.
#
# Assumes direnv/.envrc already loaded the `.#ui` dev shell. If you're
# running outside direnv, prefix with `nix develop .#ui --command`.
#
# NOTE: serve WITHOUT hot-patching — `dx serve`'s default hot-patch
# path breaks wasm builds (LinkError / subsecond panic on edit).
web: css
    cd apps/web && dx serve --web --addr 0.0.0.0 --hot-patch false

# The production web bundle, exactly as nix builds it
# (nix/modules/packages/web-bundles.nix): `[profile.wasm-release]` from
# the root Cargo.toml, DWARF dropped, and the binary split into a main
# chunk plus one lazily fetched chunk per route and per plugin app
# (needs the dev shell's dx — the #5668 fork from nix/modules/dx.nix;
# the published 0.8.0-alpha.0 panics splitting this app, see
# docs/task-webapp.md). Output: target/dx/task-app-web/release/web/public/.
# Prints the chunk sizes at the end so a size regression is visible
# without deploying.
web-release: css
    #!/usr/bin/env bash
    set -euo pipefail
    cd apps/web && env -u RUSTC_WRAPPER dx build --release --platform web \
        --debug-symbols false --wasm-split --features wasm-split
    cd ../..
    du -h target/dx/task-app-web/release/web/public/assets/*.wasm | sort -h

# The local web app against a DEPLOYED server, signed in as you.
#
# Reads TASK_LIVE_{SERVER,EMAIL,PASSWORD,NAME} from `.env` (gitignored;
# `.env.example` documents them). The wasm build bakes the server's vox
# URL and the account as the debug demo cast, so :8766 boots signed in
# through the server's issuer and every edit hot-reloads against the
# real data. Debug builds only — `TASK_DEMO_CAST` is `option_env!`
# behind `debug_assertions`.
live: css
    #!/usr/bin/env bash
    set -euo pipefail
    [ -f .env ] && set -a && . ./.env && set +a
    : "${TASK_LIVE_SERVER:?set TASK_LIVE_SERVER in .env (https://task.example.com)}"
    : "${TASK_LIVE_EMAIL:?set TASK_LIVE_EMAIL in .env}"
    : "${TASK_LIVE_PASSWORD:?set TASK_LIVE_PASSWORD in .env}"
    vox="${TASK_LIVE_SERVER/https:\/\//wss://}"; vox="${vox/http:\/\//ws://}"
    echo ">> web on http://127.0.0.1:8766 → ${vox%/}/vox as ${TASK_LIVE_EMAIL}"
    cd apps/web && exec env \
        TASK_VOX_URL_WEB="${vox%/}/vox" \
        TASK_DEMO_CAST="${TASK_LIVE_EMAIL}:${TASK_LIVE_PASSWORD}:${TASK_LIVE_NAME:-$TASK_LIVE_EMAIL}:" \
        dx serve --web --addr 127.0.0.1 --port 8766 --hot-patch false

# Regenerate every app's assets/tailwind.css from the ONE source input,
# `apps/tailwind.css`.
#
# You rarely need this: dx runs the Tailwind watcher itself (Dioxus 0.7
# detects the `tailwind.css` at each crate root), so `just web` /
# `desktop` / `mobile` already compile the sheet. This recipe is for the
# case dx isn't in the loop — a plain `cargo check --workspace`, which
# still has to find the file because `asset!()` resolves at compile time.
#
# The sheet is identical whichever way it is produced: the input sets
# `source(none)` and names every source explicitly, so the output does
# not depend on the working directory.
css:
    tailwindcss -i apps/tailwind.css -o apps/desktop/assets/tailwind.css
    tailwindcss -i apps/tailwind.css -o apps/mobile/assets/tailwind.css
    tailwindcss -i apps/tailwind.css -o apps/web/assets/tailwind.css

# Back-compat aliases — all three sheets come from the same input now.
desktop-css: css

# Native desktop window (package `task-app-desktop`). Regenerates
# tailwind first so any new utility classes in touched sources
# actually exist in the bundled stylesheet, then hot-reloads on
# subsequent source changes.
desktop: desktop-css
    cd apps/desktop && dx serve --platform desktop

# Same as `desktop` but in release mode — slower compile, snappier
# runtime; use when smoke-testing a vault for actual editing.
desktop-release: desktop-css
    cd apps/desktop && dx serve --platform desktop --release

# The shippable macOS build: a Developer-ID .dmg, notarized and
# stapled, with the sync agent inside the bundle — so installing the app
# installs background file sync (an App Store build cannot: a sandboxed
# app may not register a LaunchAgent).
#
# macOS only, and needs a "Developer ID Application" certificate in the
# keychain. `DRY_RUN=1 just dmg` stops at a signed, un-notarized image.
dmg:
    bash apps/desktop/macos/build-dmg.sh

# The sync agent on this machine: install it as a background service,
# see what it is doing, take it away again. `just sync install
# --coordinator <org endpoint id>` is the whole setup on a fresh box.
sync *ARGS="status":
    cargo run --quiet -p files-daemon --features daemon-bin --bin fts-files-daemon -- {{ARGS}}

# Regenerate apps/mobile/assets/tailwind.css (package `task-app-mobile`).
mobile-css: css

# Canonical server. Default bind is 0.0.0.0:9090; override with
# TASK_SERVER_BIND. See `.env.example` for the full env surface.
server:
    TASK_SERVER_BIND="0.0.0.0:9090" cargo run --release -p task-server

# Launch server + web side-by-side; Ctrl+C kills both. Server lines
# prefixed [srv], web lines [web].
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    TASK_SERVER_BIND="0.0.0.0:9090" \
        cargo run --release -p task-server 2>&1 | sed 's/^/[srv] /' &
    just web 2>&1 | sed 's/^/[web] /' &
    wait

# ── Dev seed ─────────────────────────────────────────────────────────────

# A throwaway LOCAL multi-org dev vault with a known owner login + demo
# data (including a Files root with version history + a divergence, for
# the issue #267 UI). Isolated from prod — its own $TASK_DATA_ROOT under
# ~/.local/share/task-dev-seed. See scripts/dev-seed.sh for env knobs.
#
#   just dev-seed          # build + seed (tops up; idempotent)
#   just dev-seed fresh    # wipe + reseed from scratch
#   just dev-seed serve    # run task-server against the dev vault (:9099)
#   just dev-seed web      # run the web app pointed at it (:8765)
dev-seed *ARGS="seed":
    scripts/dev-seed.sh {{ARGS}}

# The example studio, running: two companies on two servers, each with
# its own iroh endpoint, federating by endpoint id. This is
# `examples/studio` — the tree the integration suite reads and the four
# people it hires — planted on disk and served, so the world the tests
# assert against is a world you can sign into.
#
# `dev-seed` is the other one: several orgs in one process, for
# eyeballing multi-org UI. Use this when the thing under test is two
# companies who are not members of each other.
#
#   just demo              # build + plant both orgs (idempotent)
#   just demo fresh        # wipe + replant
#   just demo serve        # both servers (:9101 ACME, :9102 VNT)
#   just demo web          # the web app pointed at ACME (:8766)
#   just demo desktop      # the desktop app pointed at ACME
#   just demo daemon       # a laptop: the sync agent replicating ACME
#   just demo ids          # each org's endpoint id
#   just demo telemetry    # local Grafana/Tempo/Loki/Prom; serve+desktop attach
demo *ARGS="plant":
    scripts/demo.sh {{ARGS}}

# ── Build & Test ─────────────────────────────────────────────────────────

# All recipes assume the dev shell is already loaded (`.envrc` does
# `use flake`, so direnv handles this on `cd`). On hosts without
# direnv, run `nix develop` first, or prefix any recipe with
# `nix develop . --command just <recipe>`. The root flake exposes `default`, `ci`, and
# `reaper-test` — there is no `.#ui` or `.#playwright` shell; the
# default shell already carries tailwindcss, dx, and
# PLAYWRIGHT_BROWSERS_PATH.
check:
    cargo check --workspace

build:
    cargo build --workspace

test:
    cargo test --workspace

# Browser tests — Playwright. Run inside the dev shell
# so Chromium + node come from Nix (it sets
# PLAYWRIGHT_BROWSERS_PATH):
#
#   nix develop . --command just test-browser
#
# First run does a `pnpm install` for @playwright/test (the repo
# commits pnpm-lock.yaml, not package-lock.json — no browser
# download either: the shell's `PLAYWRIGHT_BROWSERS_PATH` points
# at nixpkgs's playwright-driver.browsers). Then runs the suite,
# booting `dx serve` automatically via playwright.config.js's
# webServer block.
#
# IMPORTANT: `dx serve` hot-patch DOES NOT pick up new RSX
# attribute additions (id=, data-testid=) — only function-body
# changes. If a UI selector test fails on "element not found"
# after you added a new attribute, use `just test-browser-fresh`
# below to force a clean dx-serve restart.
test-browser:
    cd apps/tests/playwright && pnpm install --silent && pnpm exec playwright test

# Browser tests with a guaranteed-fresh `dx serve`. Sets `CI=1` so
# playwright.config.js's `reuseExistingServer` is false; any
# existing dev server (default :9100, override with PW_PORT) is
# killed first so the new one boots from scratch. Use this when:
#   - you added a new RSX attribute (`id=`, `data-testid=`) and
#     `just test-browser` finds the old DOM (hot-patch gotcha)
#   - you're hunting a sync regression and want a known-clean
#     server doc per test run
# Takes several minutes longer than `just test-browser` because it
# rebuilds the wasm bundle from cold.
test-browser-fresh:
    pkill -f "dx serve" || true
    pkill -f "target/release/task-server" || true
    sleep 1
    CI=1 just test-browser

# Multiplayer conformance suites (apps/tests/multiplayer/): 5-way editor
# convergence + 20-peer presence churn against an ISOLATED stack —
# its own task-server (port 18091, throwaway TASK_DATA_ROOT, seeded
# org + dev accounts) and a statically-served wasm bundle baked with
# TASK_VOX_URL_WEB pointing at it. Never touches the dev server on
# :18080. See apps/tests/multiplayer/README.md for the suite status table
# and the current findings. ~5 min warm, longer on a cold build.
mp-test *ARGS:
    nix develop . --command apps/tests/multiplayer/run.sh {{ARGS}}

# ── Lint / format / CI ───────────────────────────────────────────────────

fmt:
    cargo fmt --all

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

ci:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo nextest run --workspace --profile ci

# ── Git hooks (capn) ─────────────────────────────────────────────────────

# Install the capn pre-commit + pre-push hooks. Run once per clone.
install-hooks:
    ./.githooks/install.sh

# Run capn pre-commit checks manually (without committing).
capn-precommit:
    capn

# Run capn pre-push checks manually (without pushing).
capn-prepush:
    capn pre-push

# ── Releases / changelog ─────────────────────────────────────────────────

# Regenerate CHANGELOG.md from conventional commits.
changelog:
    git cliff -o CHANGELOG.md

# Preview release notes for the next bump (no file write).
changelog-preview:
    git cliff --unreleased

# ── Aliases ──────────────────────────────────────────────────────────────

alias c := check
alias b := build
alias t := test

# ── Deploy ───────────────────────────────────────────────────────────────

# Build task-cli (release) and ship it to starcommand for the
# task-email-watcher systemd service. The binary is placed at
# /var/lib/task-watcher/bin/task and the watcher is restarted.
#
# Called automatically from ~/.starcommand/justfile `deploy`, so
# `just deploy` in starcommand does the whole pipeline.
deploy-task-watcher host="root@192.168.0.106" remote="/var/lib/task-watcher/bin/task":
    cargo build --release -p task-cli
    scp target/release/task {{host}}:{{remote}}.new
    ssh {{host}} 'install -o task-watcher -g task-watcher -m 0755 {{remote}}.new {{remote}} && rm -f {{remote}}.new && systemctl restart task-email-watcher.service && sleep 2 && systemctl status task-email-watcher.service --no-pager | head -8'
