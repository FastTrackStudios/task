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
    bash scripts/architect-ui-link.sh
    tailwindcss -i apps/tailwind.css -o apps/desktop/assets/tailwind.css
    tailwindcss -i apps/tailwind.css -o apps/mobile/assets/tailwind.css
    tailwindcss -i apps/tailwind.css -o apps/web/assets/tailwind.css

# Back-compat aliases — all three sheets come from the same input now.
desktop-css: css

# Regenerates tailwind first so any new utility classes in touched
# sources exist in the bundled stylesheet. On macOS it runs as a real
# "Task" bundle — name, bundle id and icon — via
# apps/desktop/macos/dev-app.sh, without hot reload (`desktop-hot` keeps
# hot reload and runs the bare `task-app-desktop` binary); elsewhere
# it is `dx serve`.
#
# The native desktop app, as "Task" with its icon.
desktop: desktop-css
    #!/usr/bin/env bash
    if [ "$(uname)" = "Darwin" ]; then exec bash apps/desktop/macos/dev-app.sh; fi
    cd apps/desktop && exec dx serve --platform desktop

# The desktop app under `dx serve`, hot reloading, as a bare binary.
desktop-hot: desktop-css
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

# The iPhone app on the booted simulator, signed in as the demo cast
# against `just demo serve`. `--no-build` reinstalls the last build;
# `--sim <udid>` picks a simulator. See apps/mobile/ios/build-ios.sh.
mobile-sim *ARGS:
    bash apps/mobile/ios/build-ios.sh {{ARGS}}

# Attach the macOS app (signed, notarized) to the release `v<VERSION>`
# (waits for the deploy to create it): a DMG for a first install, and the
# zip + manifest installed copies update themselves from
# (apps/desktop/src/updater.rs). Run on the Mac holding the signing keys;
# CI does the same on every tag (task-macos.yml).
release-desktop VERSION:
    bash apps/desktop/macos/release.sh {{VERSION}}

# ── Releases ─────────────────────────────────────────────────────────────
#
# Fleet flow v4 (the `release-workflow` fleet skill is the source of truth;
# Task's specifics: skills/release-workflow.md). `main` is the only
# long-lived branch: every change is a squash-merged PR carrying a
# cargo-rail change file (`.changes/*.md`). A release is a TAG
# `vYY.M.MICRO` (CalVer, computed): `release-prepare` cuts `release/pending`
# (one commit: version + CHANGELOG from the change files, which it
# deletes), its PR squash-merges into main, and `release-tag` pushes the
# tag once the full suite on that commit is green. The tag deploys the
# server + web, uploads TestFlight builds, and publishes the GitHub Release
# carrying everything the version ships (node binaries, web bundle, macOS
# and Linux desktop apps). Production = the latest `v*` tag.

# The product version (server, web, iOS, macOS, Linux desktop).
version:
    @bash scripts/release.sh version

# Latest tag, what main holds since, the pending change files, the next
# (computed) version, and the migrations / moved git deps it would carry.
release-plan:
    bash scripts/release.sh plan

# Cut `release/pending` from origin/main: CalVer bump, CHANGELOG section
# from `.changes/*.md` (deleted), ONE commit `chore(release): v<next>`.
# ARGS: an explicit X.Y.Z (rare; must sort above the latest tag, and the
# commit records the override), and/or `--on release/vX.Y.Z` for a hotfix.
release-prepare *ARGS:
    bash scripts/release.sh prepare {{ARGS}}

# On release/pending: push it and open its PR (squash) into the branch it
# was cut from, with the changelog and migrations as the body.
release-pr:
    bash scripts/release.sh pr

# After the release PR merged, its full suite is green and images.yml
# pushed its images to GHCR (it refuses otherwise): tag v<version> on the
# merge commit. THE TAG DEPLOYS. Pushes as whoever you
# are: use the fts-forge bot or a person, never a workflow token (that
# starts no workflow). A hotfix tag also opens the forward-port PR.
release-tag *VERSION:
    bash scripts/release.sh tag {{VERSION}}

# Hotfix while main can't ship (fix on main FIRST): release/v<latest tag>
# from the tag, fix/<NAME> with the commits cherry-picked (-x), PR into it.
# NAME: sta-123-slug | no-issue-slug. Then release-prepare --on it.
release-hotfix NAME +COMMITS:
    bash scripts/release.sh hotfix {{NAME}} {{COMMITS}}

# One version's section of CHANGELOG.md (default: the current version).
release-notes *VERSION:
    @bash scripts/release.sh notes {{VERSION}}

# Every PR that changes a crate carries a change file (`cargo rail change
# add <crates> --bump none|patch|minor|major -m "…"`). This is the CI gate:
# against BASE when given (CI passes the PR's base sha), else the
# merge-base with main.
ci-changes BASE="":
    if [ -n "{{BASE}}" ]; then cargo rail change check --since "{{BASE}}"; else cargo rail change check --merge-base; fi

# The full suite's lint half, locally: everything `just ci` runs plus docs
# (rustdoc -D warnings).
ci-release: ci ci-docs

# The typing test: every markdown construct typed into the app's note
# view on the booted simulator through the iOS keyboard's input calls;
# text, caret and rendering checked, a screenshot per case.
ios-typing *ARGS:
    bash apps/mobile/ios/typing-test.sh {{ARGS}}

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
#   just demo federate     # the two servers subscribe to each other's sources
#   just demo ids          # each org's endpoint id
#   just demo telemetry    # local Grafana/Tempo/Loki/Prom; serve+desktop attach
demo *ARGS="plant":
    scripts/demo.sh {{ARGS}}

# Sync a YouTube channel's message videos into the deployed org as
# sermon resources (video + captions + scripture backlinks). Reads
# TASK_LIVE_SERVER from .env; the CLI session must already be signed in
# there (skills/live-dev.md §3). Extra args pass through
# (`--limit 5`, `--since 2026-01-01`, `--dry-run`). Channel / org /
# folder come from SERMON_CHANNEL / SERMON_ORG / SERMON_FOLDER, which
# default to Crossroads. Cron-friendly: exits 0 once the listing worked.
# skills/sermon-sync.md has the systemd timer.
sermon-sync *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    [ -f .env ] && set -a && . ./.env && set +a
    : "${TASK_LIVE_SERVER:?set TASK_LIVE_SERVER in .env (https://task.example.com)}"
    export TASK_YTDLP="${TASK_YTDLP:-$(nix shell nixpkgs#yt-dlp -c which yt-dlp)}"
    exec cargo run --quiet -p task-cli -- --server "$TASK_LIVE_SERVER" \
        resources sermons sync "${SERMON_CHANNEL:-https://www.youtube.com/@CrossroadsCA}" \
        --org "${SERMON_ORG:-codywright}" --folder "${SERMON_FOLDER:-crossroads}" {{ARGS}}

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

# One process per test, as the gate runs them: the integration binary's
# tests set process-wide environment the server reads (some of it per
# request — see apps/server/tests/it/support's env_lock), which only
# nextest's isolation makes sound. Doctests are cargo test's alone.
test:
    cargo nextest run --workspace
    cargo test --workspace --doc

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

clippy: ci-clippy

# The `ci-*` recipes ARE the PR gate's check set — checks.yml invokes
# these same recipes, one per step, so what you run before pushing and
# what the gate runs cannot drift.
#
# The order is the gate's order and it is deliberate: nextest needs full
# CODEGEN of every dependency, clippy needs only their METADATA, so the
# test build leaves behind the rlibs everything after it reuses. Running
# clippy first would leave the codegen to pay for anyway.

ci-fmt:
    cargo fmt --all -- --check

# Task's token sheet is architect-ui's, imported through apps/.architect-ui
# (no vendored copy). Fails on drift: a re-vendored copy, an import that
# no longer goes through the link, or a link left on another pin. Depends
# on `css` so the sheet was also compiled from that link.
ci-architect-css: css
    bash scripts/architect-ui-link.sh --check

ci-manifests:
    cargo metadata --locked --format-version 1 > /dev/null

# Crate-layout boundaries + ratcheted idioms (boundaries.toml), checked by
# architect's xtask at a pinned release tag. Resolves graphs, compiles nothing
# of Task's. Bump the tag with architect releases.
architect_xtask_tag := "v0.9.27"
ci-boundaries:
    #!/usr/bin/env bash
    set -euo pipefail
    bin="${CI_CACHE_DIR:-$HOME/.cache}/architect-xtask/{{architect_xtask_tag}}"
    if [ ! -x "$bin/bin/xtask" ]; then
        cargo install --locked --quiet --git https://github.com/FastTrackStudios/architect \
            --tag {{architect_xtask_tag}} xtask --root "$bin"
    fi
    "$bin/bin/xtask" boundaries

# One version everywhere: every shipped artifact takes the release's
# version from `[workspace.package]` (static; compiles nothing). The
# built binaries are checked where they are built: the deploy and
# node-binaries.yml run `scripts/check-versions.sh --bin`.
ci-versions:
    bash scripts/check-versions.sh

# `TASK_CI_TEST_THREADS` lets one runner ask for fewer test threads than
# the profile's 16 (the workflow sets it for the Mac Pro VM runner, STA-369:
# at 16 two tests that dial by endpoint id time out under load, at 8 the
# whole suite passes in ~2.5 min). `--test-threads` beats the
# `NEXTEST_TEST_THREADS` the runner unit exports. Unset, it is the
# profile's own 16, so nothing changes elsewhere.
ci-tests: css
    #!/usr/bin/env bash
    # nextest's report goes to a FILE, then a digest to the step's pipe. On
    # the Mac Pro VM runner nextest died with exit 110, "error reporting
    # results: error writing to output", at the first failing test's output
    # (run after run, 1250-1460 tests in, the rest "not run due to reporting
    # error"): writing a failure's captured log to the runner's stdout pipe
    # failed. A regular file cannot do that. The full log is the step's
    # artifact (`TASK_CI_TEST_LOG`); the step shows the summary and the tail.
    set -uo pipefail
    log="${TASK_CI_TEST_LOG:-${TMPDIR:-/tmp}/nextest-ci.log}"
    mkdir -p "$(dirname "$log")"
    # TASK_CI_TEST_FILTER: a nextest filterset from the plan job (only the
    # packages a PR affects, scripts/ci-rail-plan.py). Empty = every test.
    # The BUILD stays --workspace either way so the warm target dir keeps
    # one artifact set; only which tests run narrows.
    filter=()
    [ -n "${TASK_CI_TEST_FILTER:-}" ] && filter=(-E "$TASK_CI_TEST_FILTER" --no-tests=pass)
    cargo nextest run --profile ci --workspace "${filter[@]}" --test-threads {{ env("TASK_CI_TEST_THREADS", "16") }} >"$log" 2>&1
    rc=$?
    { grep -E '^ *(FAIL|TRY [0-9]+ (FAIL|TMT)|SIGSEGV|SIGABRT|LEAK|TIMEOUT)|Summary \[|tests were not run|^error' "$log" | head -n 200
      echo "── tail of $log (full log: the step's nextest-ci artifact) ──"; tail -n 60 "$log"; } || true
    exit "$rc"

ci-clippy: css
    cargo clippy --workspace --all-targets -- -D warnings

# The copy lints at WARN (docs/perf/zero-copy-audit.md): a report, not a
# gate. `ci-clippy` denies every warning, so these four cannot sit in
# `[workspace.lints]` at warn until the whole workspace is clean;
# instead each crate denies them in its own root (`#![deny(...)]`) as it
# is cleaned, and this recipe counts what is left everywhere else.
# `just clippy-copies -p task-server` narrows it to one package.
clippy-copies *ARGS:
    cargo clippy {{ if ARGS == "" { "--workspace" } else { ARGS } }} --all-targets -- \
        -W clippy::redundant_clone -W clippy::implicit_clone \
        -W clippy::needless_pass_by_value -W clippy::large_types_passed_by_value \
        2>&1 | grep -oE 'rust-clippy/[^ ]*#(redundant_clone|implicit_clone|needless_pass_by_value|large_types_passed_by_value)' \
        | sed 's/.*#//' | sort | uniq -c | sort -rn || true

# The wasm32 check, one package at a time: every row of
# .config/rail-wasm.json by default, or the PACKAGES `cargo rail plan`
# selected (`ci-local`). `task-app-web` is the browser app. The browser
# dial (`task-dial`) is what an app outside this repo depends on, so it is
# checked on its own too: `task-app-web` would keep compiling if it grew a
# native-only dependency behind a cfg. Likewise `files-client`, the store
# Files client apps use. A new row goes in the catalog, which this reads.
ci-wasm *PACKAGES: css
    #!/usr/bin/env bash
    set -euo pipefail
    pkgs="{{PACKAGES}}"
    [ -n "$pkgs" ] || pkgs="$(python3 -c 'import json; print(" ".join(v["cargo_roots"][0]["package"] for v in json.load(open(".config/rail-wasm.json"))["variants"]))')"
    for p in $pkgs; do
        cargo check --target wasm32-unknown-unknown -p "$p"
    done

ci-docs: css
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# Everything the full run after a merge checks, in its order (all the
# tests, not just the affected ones). The PR gate is `ci-local`, below.
# (`ci-docs` runs in no CI job since 2026-10-24, too slow for what it
# catches; `just ci-release` adds it, or run it by hand. `ci-changes` is
# separate: it needs cargo-rail.)
ci: ci-fmt ci-manifests ci-versions ci-architect-css ci-tests ci-clippy ci-wasm

# ── Target dir: keep the latest build of each kind ───────────────────────
#
# A shared target dir only ever grows: every new dependency graph (a
# lockfile bump, another branch's features) leaves a whole new set of
# units beside the old. `build-kind` runs ANY build — cargo, a `just`
# recipe, dx — and records which units it used, fresh or rebuilt, as the
# latest build of <kind>; `sweep` deletes what no kind's latest build
# used. See scripts/target-sweep.py and docs/agents/build-strategy.md.
#
#   just build-kind check cargo check --workspace --all-targets
#   just build-kind desktop just desktop
#   just sweep --dry-run
build-kind kind +cmd:
    python3 scripts/target-sweep.py run {{kind}} -- {{cmd}}

sweep *args:
    python3 scripts/target-sweep.py sweep {{args}}

# The local/canonical policy: recorded kinds + anything any build (dx
# serve, the IDE, a bare cargo run) used in the last 7 days; rust-analyzer's
# flycheck dir goes whole past 40 GB. Safe to run from a timer.
sweep-local *args:
    python3 scripts/target-sweep.py sweep --keep-days 7 --flycheck-max-gb 40 {{args}}

sweep-status:
    python3 scripts/target-sweep.py status

# ── Git hooks ────────────────────────────────────────────────────────────

# Install the git hooks (prek.toml: rustfmt/shellcheck/actionlint on the
# staged files at commit, `ci-local` at push) into .git/hooks. Run once
# per clone; worktrees share them. A clone from before prek pointed
# core.hooksPath at .githooks (capn), which would shadow prek's shims.
install-hooks:
    -git config --unset core.hooksPath
    prek install --force

# Run the commit hooks on every file (prek's `--all-files`), e.g. after
# changing prek.toml.
hooks-all:
    prek run --all-files

# Move nix/modules/prek.nix to prek's newest release (or VERSION, e.g.
# 0.5.7): rewrites the version and every platform's sha256 from the
# release's own `.sha256` files. Then re-enter the shell and
# `just install-hooks` (the shims name the prek they were installed with).
prek-bump VERSION="":
    #!/usr/bin/env bash
    set -euo pipefail
    v="{{VERSION}}"
    if [ -z "$v" ]; then
        v="$(curl -fsSL https://api.github.com/repos/j178/prek/releases/latest | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')"
    fi
    [ -n "$v" ] || { echo "could not read prek's latest release" >&2; exit 1; }
    f=nix/modules/prek.nix
    sed -i "s/version = \"[^\"]*\";/version = \"$v\";/" "$f"
    for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin; do
        sum="$(curl -fsSL "https://github.com/j178/prek/releases/download/v$v/prek-$t.tar.gz.sha256" | cut -d' ' -f1)"
        [[ "$sum" =~ ^[0-9a-f]{64}$ ]] || { echo "no sha256 for $t at v$v" >&2; exit 1; }
        sed -i "s|target = \"$t\"; sha256 = \"[0-9a-f]*\"|target = \"$t\"; sha256 = \"$sum\"|" "$f"
    done
    git diff --stat -- "$f"
    echo "prek -> $v"

# The PR gate's clippy + tests + wasm, for what the change affects (`cargo
# rail plan` from BASE, default the merge-base with origin/main). The
# pre-push hook runs this and marks the pushed commits `Local CI`.
ci-local BASE="":
    bash scripts/ci-local.sh {{BASE}}

# The route pins in .config/rail-cases.toml: for a change to each path,
# the work `cargo rail plan` must require or skip (CI's pr-checks runs it).
# Plans synthetic commits on top of HEAD; touches nothing.
rail-cases:
    cargo rail plan --cases .config/rail-cases.toml

# Record rail's observed-input evidence for HEAD (clean, normally main
# after a pull), so `ci-local` against this commit narrows a docs or
# fixture change instead of running everything. Kept in the git dir,
# keyed by commit. Costs one test build + one clippy, both warm.
rail-evidence: css
    #!/usr/bin/env bash
    set -euo pipefail
    dir="$(git rev-parse --git-common-dir)/rail-evidence"
    mkdir -p "$dir"
    ev="$dir/$(git rev-parse HEAD).json"
    cargo rail plan evidence --work cargo.test -o "$ev" -- test --workspace --locked --no-run
    cargo rail plan evidence --work cargo.clippy -o "$ev" -- clippy --workspace --all-targets -- -D warnings
    echo "rail evidence: $ev"
    # Incomplete evidence narrows nothing: rail widens every change to the
    # whole workspace. The usual cause is an untracked file a compile read,
    # e.g. a Finder .DS_Store under examples/studio, which task-server
    # embeds whole (`include_dir!`).
    python3 - "$ev" <<'EOF'
    import json, sys
    for work, w in json.load(open(sys.argv[1]))["work"].items():
        if not w["complete"]:
            print(f"rail evidence: {work} is INCOMPLETE ({', '.join(w['bypasses'])}); it will not narrow", file=sys.stderr)
    EOF
    untracked="$(git ls-files --others --exclude-standard --ignored examples/studio)"
    [ -z "$untracked" ] || printf 'rail evidence: untracked files under examples/studio (embedded by task-server):\n%s\n' "$untracked" >&2

# Pre-warm THEBATTLESHIP's Task runner pools, so the next merge or release
# compiles only what it changed instead of starting cold. Use it after
# anything that leaves a pool cold: a toolchain bump, a lockfile overhaul, a
# wiped cache. Point REF at the branch that makes the change BEFORE merging
# it, and the merge itself lands warm.
#
#   main     task-main's dev target dirs: a build-only checks.yml run (lint,
#            the test build, wasm; no tests run, no status posted)
#   release  task-release's release target dir: a build-only images.yml run
#            (the release server and the web bundle; nothing pushed)
#
#   just pre-warm-ci                        both pools, from main
#   just pre-warm-ci main ci/nightly-bump   task-main, from a branch
#   just pre-warm-ci both main fresh        empty task-main's dirs first (a
#                                           new compiler never reuses the
#                                           old artifacts; this drops them)
#
# The pool builds it, in its own environment and at its own paths: a copy
# of a local target dir would not do. Local builds differ in flags (mold,
# CARGO_INCREMENTAL), build scripts bake the target dir's absolute path into
# their outputs, and a runner checkout older than copied artifacts would
# trust stale units.
pre-warm-ci POOL="both" REF="main" FRESH="":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ POOL }}" in
        both | main | release) ;;
        *) echo "pool: main, release or both (got {{ POOL }})" >&2; exit 2 ;;
    esac
    fresh=false
    [ "{{ FRESH }}" = "fresh" ] && fresh=true
    n=0
    if [ "{{ POOL }}" != release ]; then
        gh workflow run checks.yml --ref "{{ REF }}" -f prewarm=true -f fresh="$fresh"
        echo "pre-warm: task-main (checks.yml) from {{ REF }} (fresh=$fresh)"
        n=$((n + 1))
    fi
    if [ "{{ POOL }}" != main ]; then
        gh workflow run images.yml --ref "{{ REF }}" -f prewarm=true
        echo "pre-warm: task-release (images.yml) from {{ REF }}"
        n=$((n + 1))
    fi
    sleep 5
    gh run list --event workflow_dispatch --limit "$n" \
        --json databaseId,workflowName,status,headBranch,url -q '.[] | "\(.databaseId)  \(.workflowName)  \(.status)  \(.headBranch)  \(.url)"'
    echo "watch: gh run watch <id>"

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
