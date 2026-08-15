#!/usr/bin/env bash
# Multiplayer conformance suites — build + run (tracked issue dd824506).
#
# Builds everything the harness needs, then runs Playwright:
#   1. task-server + task-cli (debug — runtime perf is irrelevant)
#   2. the dx web bundle with TASK_VOX_URL_WEB baked to the ISOLATED
#      test server port (the vox URL is a compile-time constant on
#      wasm — see crates/task/ui/src/vox_session.rs)
#   3. pnpm install (first run only)
#   4. playwright test
#
# Running `dx build` to completion HERE (not inside a webServer
# timeout) is the warmup that absorbs the dx cold-build flake the
# Editor repo documents — by the time Playwright starts, the bundle
# is static bytes served by serve.js.
#
# Run inside the repo dev shell so the pinned Playwright browsers and
# the wasm C toolchain resolve (there is no separate .#playwright
# shell — the default one carries PLAYWRIGHT_BROWSERS_PATH):
#   nix develop --command apps/task/tests/multiplayer/run.sh
#
# Env knobs:
#   MP_SERVER_PORT (default 18091)  isolated task-server port
#   MP_WEB_PORT    (default 18092)  static bundle port
#   MP_SEED                         churn schedule seed
#   MP_SKIP_BUILD=1                 reuse existing artifacts

set -euo pipefail
cd "$(dirname "$0")"
# The monorepo root. This script lives at apps/task/tests/multiplayer,
# so the root is four levels up — it was two when Task was a standalone
# repo, and the subtree import left this pointing at apps/task/.
REPO_ROOT="$(cd ../../../.. && pwd)"
MP_SERVER_PORT="${MP_SERVER_PORT:-18091}"

if [[ "${MP_SKIP_BUILD:-}" != "1" ]]; then
  # Outside the nix dev shell the wasm C toolchain is missing and
  # arborium's tree-sitter grammars silently end up as unresolved
  # `env` imports — the app then fails to even load ("Failed to
  # resolve module specifier \"env\""). Fail fast instead.
  if [[ -z "${CC_wasm32_unknown_unknown:-}" ]]; then
    echo "error: CC_wasm32_unknown_unknown is unset — run inside the dev shell:" >&2
    echo "  nix develop --command apps/task/tests/multiplayer/run.sh" >&2
    exit 1
  fi

  echo "[mp] building task-server + task-cli (debug)…"
  (cd "$REPO_ROOT" && cargo build -p task-server -p task-cli)

  echo "[mp] building the web bundle (TASK_VOX_URL_WEB=ws://127.0.0.1:${MP_SERVER_PORT}/vox)…"
  (cd "$REPO_ROOT/apps/task/web" && TASK_VOX_URL_WEB="ws://127.0.0.1:${MP_SERVER_PORT}/vox" dx build --platform web)
fi

if [[ ! -d node_modules ]]; then
  echo "[mp] installing @playwright/test…"
  pnpm install --silent
fi

export MP_SERVER_PORT
exec npx playwright test "$@"
